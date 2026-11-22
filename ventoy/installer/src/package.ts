/**
 * Repacks a Ventoy release package (the Windows `.zip` or Linux `.tar.gz`)
 * with d-pad support in its boot image, `<dir>/ventoy/ventoy.disk.img.xz`.
 * Everything else in the package is left byte for byte as it was.
 *
 * Uses the system's `xz`, mtools, `zip`/`unzip` and GNU `tar`/`gzip`
 * (Linux, or WSL on Windows). Set `SOURCE_DATE_EPOCH` for byte-identical
 * output from the same inputs.
 * @module
 */
import { basename, join } from "@std/path";
import { MtoolsFat } from "./fat.ts";
import { patchImage, type Payload } from "./stick.ts";

/** The package member that holds Ventoy's boot image. */
const IMAGE = /^(?:\.\/)?([^/]+)\/ventoy\/ventoy\.disk\.img\.xz$/;

/**
 * xz settings matching Ventoy's own image: CRC32 (the only check every Ventoy
 * decoder supports) and an 8 MiB dictionary, so decompressing needs no more
 * memory than before.
 */
const XZ = ["--check=crc32", "--lzma2=preset=9e,dict=8MiB"];

/** What to add, and what to say about it. */
export interface PackageOptions {
  /** claw-chainload.efi and claw-dpad.efi. */
  payload: Payload;
  /** Text of the notice file added next to Ventoy's files. */
  notice: string;
  /** Name of that file inside the package's top folder. */
  noticeName: string;
  /** Extra files for the top folder (e.g. Ventoy's license), by name. */
  extra?: Record<string, Uint8Array>;
}

/** Package formats this module can repack. */
type Kind = "zip" | "tar.gz";

/** Runs a command; returns stdout, throwing with stderr if it fails. */
async function run(cmd: string, args: string[], cwd?: string): Promise<Uint8Array> {
  const out = await new Deno.Command(cmd, { args, cwd, stdout: "piped", stderr: "piped" })
    .output();
  if (!out.success) {
    const err = new TextDecoder().decode(out.stderr).trim();
    throw new Error(`${cmd} ${args.join(" ")}: ${err || `exit ${out.code}`}`);
  }
  return out.stdout;
}

/** Reads one member of a package. */
function extract(pkg: string, kind: Kind, member: string): Promise<Uint8Array> {
  return kind === "zip" ? run("unzip", ["-p", pkg, member]) : run("tar", ["xzOf", pkg, member]);
}

/**
 * `SOURCE_DATE_EPOCH`, when set, for reproducible output: mtools stamps the
 * image's new files with it, and the added package members get it as mtime.
 */
function sourceDate(): number | undefined {
  const value = Deno.env.get("SOURCE_DATE_EPOCH");
  return value ? Number(value) : undefined;
}

/** Tells the package format from its file name. */
export function kindOf(path: string): Kind {
  if (path.endsWith(".zip")) return "zip";
  if (path.endsWith(".tar.gz") || path.endsWith(".tgz")) return "tar.gz";
  throw new Error(`${path}: expected a Ventoy .zip or .tar.gz package`);
}

/**
 * Returns the output name for a package: `ventoy-1.1.18-windows.zip` becomes
 * `ventoy-1.1.18-claw-windows.zip`.
 */
export function patchedName(path: string): string {
  const name = basename(path);
  const m = name.match(/^(ventoy-[^-]+)-(.+)$/);
  return m ? `${m[1]}-claw-${m[2]}` : `claw-${name}`;
}

/** Finds the boot image member and the package's top folder in a member list. */
export function findImage(members: string[]): { member: string; top: string } {
  const hits = members.flatMap((m) => {
    const hit = m.match(IMAGE);
    return hit ? [{ member: m, top: hit[1] }] : [];
  });
  if (hits.length !== 1) {
    throw new Error(`expected one */ventoy/ventoy.disk.img.xz, found ${hits.length}`);
  }
  return hits[0];
}

/** Lists a package's members. */
async function list(pkg: string, kind: Kind): Promise<string[]> {
  const out = kind === "zip" ? await run("unzip", ["-Z1", pkg]) : await run("tar", ["tzf", pkg]);
  return new TextDecoder().decode(out).split("\n").filter(Boolean);
}

/**
 * Writes a patched copy of the Ventoy package `pkg` into `outDir` and returns
 * its path. The boot image gains the chainloader and driver
 * ({@linkcode patchImage}); the notice and extra files go in the top folder.
 */
export async function patchPackage(
  pkg: string,
  outDir: string,
  opts: PackageOptions,
): Promise<string> {
  const kind = kindOf(pkg);
  const { member, top } = findImage(await list(pkg, kind));
  const work = await Deno.makeTempDir({ prefix: "claw-ventoy-" });
  try {
    // Unpack and patch the boot image.
    const img = join(work, "ventoy.disk.img");
    await Deno.writeFile(`${img}.xz`, await extract(pkg, kind, member));
    await Deno.writeFile(img, await run("xz", ["-dc", `${img}.xz`]));
    await patchImage(new MtoolsFat(img), opts.payload);

    // Stage the replaced and added files under the member's own path.
    const stage = join(work, "stage");
    const prefix = member.startsWith("./") ? "./" : "";
    const imageDir = join(stage, top, "ventoy");
    await Deno.mkdir(imageDir, { recursive: true });
    await Deno.writeFile(join(imageDir, "ventoy.disk.img.xz"), await run("xz", [...XZ, "-c", img]));
    const added = { [opts.noticeName]: new TextEncoder().encode(opts.notice), ...opts.extra };
    for (const [name, data] of Object.entries(added)) {
      await Deno.writeFile(join(stage, top, name), data);
    }
    const paths = [member, ...Object.keys(added).map((n) => `${prefix}${top}/${n}`)];
    const epoch = sourceDate();
    if (epoch !== undefined) {
      for (const p of paths) await Deno.utime(join(stage, p), epoch, epoch);
    }

    // Copy the package and swap the staged files in.
    await Deno.mkdir(outDir, { recursive: true });
    const out = join(outDir, patchedName(pkg));
    if (kind === "zip") {
      await Deno.copyFile(pkg, out);
      await run("zip", ["-X", "-q", Deno.realPathSync(out), ...paths], stage);
    } else {
      const tar = join(work, "package.tar");
      await Deno.writeFile(tar, await run("gzip", ["-dc", pkg]));
      await run("tar", ["--delete", "-f", tar, member]);
      const owner = ["--owner=root:0", "--group=root:0", "--mode=u=rw,go=r"];
      await run("tar", ["-rf", tar, "-C", stage, ...owner, ...paths]);
      await Deno.writeFile(out, await run("gzip", ["-9n", "-c", tar]));
    }
    return out;
  } finally {
    await Deno.remove(work, { recursive: true });
  }
}

/** Reads Ventoy's version (`<dir>/ventoy/version`) from a package. */
export async function versionOf(pkg: string): Promise<string> {
  const kind = kindOf(pkg);
  const { member } = findImage(await list(pkg, kind));
  const file = member.replace(/ventoy\.disk\.img\.xz$/, "version");
  return new TextDecoder().decode(await extract(pkg, kind, file)).trim();
}
