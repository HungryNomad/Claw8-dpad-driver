import { assert, assertEquals, assertThrows } from "@std/assert";
import { join } from "@std/path";
import { hasMtools, MtoolsFat } from "../src/fat.ts";
import { findImage, kindOf, patchedName, patchPackage, versionOf } from "../src/package.ts";
import { PATHS } from "../src/stick.ts";

const enc = (s: string) => new TextEncoder().encode(s);
const dec = (b: Uint8Array) => new TextDecoder().decode(b);

/** Runs a command in `cwd` and returns stdout; throws if it fails. */
async function run(cmd: string, args: string[], cwd?: string): Promise<Uint8Array> {
  const out = await new Deno.Command(cmd, { args, cwd, stdout: "piped", stderr: "piped" })
    .output();
  if (!out.success) throw new Error(`${cmd}: ${dec(out.stderr)}`);
  return out.stdout;
}

/** Whether every command exists. */
async function have(...cmds: string[]): Promise<boolean> {
  for (const cmd of cmds) {
    try {
      await new Deno.Command(cmd, { args: ["--version"], stdout: "null", stderr: "null" })
        .output();
    } catch {
      return false;
    }
  }
  return true;
}

/**
 * Builds a miniature Ventoy package in `dir`: a version file, a tool, and a
 * 2 MiB VTOYEFI image holding shim, GRUB and grub.cfg. Returns its folder.
 */
async function ventoyTree(dir: string): Promise<string> {
  const top = join(dir, "ventoy-9.9.9");
  await Deno.mkdir(join(top, "ventoy"), { recursive: true });
  await Deno.writeTextFile(join(top, "ventoy", "version"), "9.9.9\n");
  await Deno.writeTextFile(join(top, "Ventoy2Disk.sh"), "#!/bin/sh\n");
  await Deno.chmod(join(top, "Ventoy2Disk.sh"), 0o755);
  const img = join(dir, "ventoy.disk.img");
  await run("mkfs.fat", ["-n", "VTOYEFI", "-C", img, "2048"]);
  const fat = new MtoolsFat(img);
  await fat.write(PATHS.boot, enc("shim"));
  await fat.write(PATHS.grub, enc("grub"));
  await fat.write("/grub/grub.cfg", enc("menuentry"));
  await Deno.writeFile(join(top, "ventoy", "ventoy.disk.img.xz"), await run("xz", ["-c", img]));
  return top;
}

const payload = { loader: enc("claw-chainload: test"), driver: enc("driver") };
const opts = { payload, notice: "notice\n", noticeName: "NOTICE.txt" };

/** Reads the boot image out of a patched package and opens it. */
async function patchedImage(pkg: string, member: string, dir: string): Promise<MtoolsFat> {
  const args = pkg.endsWith(".zip") ? ["-p", pkg, member] : ["xzOf", pkg, member];
  const xz = await run(pkg.endsWith(".zip") ? "unzip" : "tar", args);
  const img = join(dir, "check.img");
  await Deno.writeFile(`${img}.xz`, xz);
  await run("xz", ["-df", `${img}.xz`]);
  return new MtoolsFat(img);
}

Deno.test("package names and members", () => {
  assertEquals(patchedName("dl/ventoy-1.1.18-windows.zip"), "ventoy-1.1.18-claw-windows.zip");
  assertEquals(kindOf("a.tar.gz"), "tar.gz");
  assertThrows(() => kindOf("a.iso"));
  assertEquals(findImage(["./v-1/", "./v-1/ventoy/ventoy.disk.img.xz", "./v-1/x"]), {
    member: "./v-1/ventoy/ventoy.disk.img.xz",
    top: "v-1",
  });
  assertThrows(() => findImage(["v-1/ventoy/ventoy_4k.disk.img.xz"]));
});

for (const kind of ["zip", "tar.gz"] as const) {
  Deno.test({
    name: `patchPackage repacks a ${kind} with only the image changed`,
    ignore: !(await hasMtools()) || !(await have("mkfs.fat", "xz", "zip", "unzip", "tar")),
    async fn() {
      const dir = await Deno.makeTempDir();
      await ventoyTree(dir);
      const pkg = join(dir, `ventoy-9.9.9-test.${kind}`);
      if (kind === "zip") await run("zip", ["-qr", pkg, "ventoy-9.9.9"], dir);
      else await run("tar", ["czf", pkg, "./ventoy-9.9.9"], dir);

      assertEquals(await versionOf(pkg), "9.9.9");
      const out = await patchPackage(pkg, join(dir, "out"), opts);
      assertEquals(out, join(dir, "out", `ventoy-9.9.9-claw-test.${kind}`));

      const prefix = kind === "zip" ? "" : "./";
      const fat = await patchedImage(out, `${prefix}ventoy-9.9.9/ventoy/ventoy.disk.img.xz`, dir);
      assertEquals(await fat.read(PATHS.grub), payload.loader);
      assertEquals(await fat.read(PATHS.movedGrub), enc("grub"));
      assertEquals(await fat.read(PATHS.boot), enc("shim"));

      const list = dec(
        await run(kind === "zip" ? "unzip" : "tar", kind === "zip" ? ["-Z", out] : ["tvzf", out]),
      );
      assert(list.includes("ventoy-9.9.9/NOTICE.txt"));
      assert(/rwxr-xr-x.*Ventoy2Disk\.sh/.test(list), "the tool stays executable");
      assertEquals(list.match(/ventoy\.disk\.img\.xz/g)?.length, 1);
      await Deno.remove(dir, { recursive: true });
    },
  });
}
