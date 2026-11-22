import { assert, assertEquals, assertRejects } from "@std/assert";
import { join } from "@std/path";
import { DirFat } from "../src/fat.ts";
import { install, patchImage, PATHS, status, uninstall } from "../src/stick.ts";

const enc = (s: string) => new TextEncoder().encode(s);
const VENTOY = enc("ventoy shim v1");
const payload = { loader: enc("...claw-chainload: v2..."), driver: enc("driver") };

/** A temp folder laid out like Ventoy's EFI partition, plus a backup log. */
async function stick() {
  const root = await Deno.makeTempDir();
  const fat = new DirFat(root);
  await fat.write(PATHS.boot, VENTOY);
  await fat.write("/grub/grub.cfg", enc("menuentry"));
  const backups: Uint8Array[] = [];
  const backup = (b: Uint8Array) => (backups.push(b), Promise.resolve());
  return { root, fat, backups, backup };
}

Deno.test("install keeps the original next to the chainloader and backs it up", async () => {
  const { fat, backups, backup } = await stick();
  await install(fat, payload, backup);
  assertEquals(await fat.read(PATHS.boot), payload.loader);
  assertEquals(await fat.read(PATHS.original), VENTOY);
  assertEquals(await fat.read(PATHS.driver), payload.driver);
  assertEquals(backups, [VENTOY]);
  assertEquals(await status(fat), {
    ventoy: true,
    installed: true,
    original: true,
    driver: true,
    patched: false,
  });
});

Deno.test("reinstall updates files without losing the original", async () => {
  const { fat, backups, backup } = await stick();
  await install(fat, payload, backup);
  await install(fat, { ...payload, driver: enc("driver v2") }, backup);
  assertEquals(await fat.read(PATHS.original), VENTOY);
  assertEquals(await fat.read(PATHS.driver), enc("driver v2"));
  assertEquals(backups.length, 1);
});

Deno.test("install after a Ventoy update adopts the new loader", async () => {
  const { fat, backup } = await stick();
  await install(fat, payload, backup);
  await fat.write(PATHS.boot, enc("ventoy shim v2"));
  await install(fat, payload, backup);
  assertEquals(await fat.read(PATHS.original), enc("ventoy shim v2"));
});

Deno.test("uninstall restores the stick exactly", async () => {
  const { root, fat, backup } = await stick();
  await install(fat, payload, backup);
  await uninstall(fat);
  assertEquals(await fat.read(PATHS.boot), VENTOY);
  assert(!(await fat.exists(PATHS.original)));
  assert(!(await fat.exists(PATHS.driverDir)));
  assertEquals([...Deno.readDirSync(join(root, "EFI"))].map((e) => e.name), ["BOOT"]);
});

Deno.test("install refuses non-Ventoy partitions and wrong loaders", async () => {
  const { fat, backup } = await stick();
  await assertRejects(() => install(fat, { ...payload, loader: enc("other") }, backup));
  await fat.remove("/grub/grub.cfg");
  await assertRejects(() => install(fat, payload, backup));
  assertEquals(await fat.read(PATHS.boot), VENTOY);
});

/** A temp folder laid out like Ventoy's boot image (shim, fbx64 and GRUB). */
async function image() {
  const { fat } = await stick();
  await fat.write(PATHS.grub, enc("ventoy grub"));
  return fat;
}

Deno.test("patchImage puts the chainloader in GRUB's place and keeps shim", async () => {
  const fat = await image();
  await patchImage(fat, payload);
  assertEquals(await fat.read(PATHS.grub), payload.loader);
  assertEquals(await fat.read(PATHS.movedGrub), enc("ventoy grub"));
  assertEquals(await fat.read(PATHS.driver), payload.driver);
  assertEquals(await fat.read(PATHS.boot), VENTOY);
  assert((await status(fat)).patched);
});

Deno.test("patchImage refuses a patched or non-Ventoy image", async () => {
  const fat = await image();
  await patchImage(fat, payload);
  await assertRejects(() => patchImage(fat, payload), Error, "already patched");
  const plain = (await stick()).fat;
  await assertRejects(() => patchImage(plain, payload), Error, "doesn't look like");
});

Deno.test("install refuses a stick made from a patched package", async () => {
  const fat = await image();
  await patchImage(fat, payload);
  await assertRejects(() => install(fat, payload, () => Promise.resolve()), Error, "patched");
  assertEquals(await fat.read(PATHS.boot), VENTOY);
});
