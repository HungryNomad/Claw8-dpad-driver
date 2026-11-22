import { assertEquals } from "@std/assert";
import { fileReader, findEfiOffset } from "../src/disk.ts";
import { hasMtools, MtoolsFat } from "../src/fat.ts";
import { install, PATHS, uninstall } from "../src/stick.ts";

const enc = (s: string) => new TextEncoder().encode(s);

/** Whether `cmd` runs. */
async function have(cmd: string): Promise<boolean> {
  try {
    return (await new Deno.Command(cmd, { args: ["--help"], stdout: "null", stderr: "null" })
      .output())
      .code !== 127;
  } catch {
    return false;
  }
}

/** A 40 MiB MBR disk image whose partition 2 is a FAT16 VTOYEFI at 4 MiB. */
async function diskImage(): Promise<string> {
  const img = await Deno.makeTempFile({ suffix: ".img" });
  await Deno.truncate(img, 40 << 20);
  const mkfs = await new Deno.Command("mkfs.fat", {
    args: ["-F", "16", "-n", "VTOYEFI", "--offset", "8192", img, String(32 << 10)],
    stdout: "null",
  }).output();
  if (!mkfs.success) throw new Error("mkfs.fat failed");
  using f = await Deno.open(img, { write: true });
  const mbr = new Uint8Array(512);
  const v = new DataView(mbr.buffer);
  mbr[446 + 16 + 4] = 0xef;
  v.setUint32(446 + 16 + 8, 8192, true);
  v.setUint32(446 + 16 + 12, 65536, true);
  mbr[510] = 0x55;
  mbr[511] = 0xaa;
  await f.write(mbr);
  return img;
}

Deno.test({
  name: "install and uninstall through mtools on a disk image",
  ignore: !(await hasMtools()) || !(await have("mkfs.fat")),
  async fn() {
    const img = await diskImage();
    const offset = await (async () => {
      using f = await Deno.open(img);
      return await findEfiOffset(fileReader(f));
    })();
    assertEquals(offset, 8192 * 512);
    const fat = new MtoolsFat(img, offset);
    await fat.write(PATHS.boot, enc("ventoy shim"));
    await fat.write("/grub/grub.cfg", enc("menuentry"));
    const payload = { loader: enc("claw-chainload: x"), driver: enc("driver") };
    await install(fat, payload, () => Promise.resolve());
    assertEquals(await fat.read(PATHS.boot), payload.loader);
    assertEquals(await fat.read(PATHS.original), enc("ventoy shim"));
    await uninstall(fat);
    assertEquals(await fat.read(PATHS.boot), enc("ventoy shim"));
    assertEquals([await fat.exists(PATHS.original), await fat.exists(PATHS.driverDir)], [
      false,
      false,
    ]);
    await Deno.remove(img);
  },
});
