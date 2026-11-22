/**
 * The d-pad chainloader on Ventoy's EFI partition, two ways:
 *
 * - {@linkcode install} / {@linkcode uninstall} edit a stick: the chainloader
 *   replaces `BOOTX64.EFI`. Every step leaves a bootable stick: the original
 *   loader is copied aside and verified before it is replaced.
 * - {@linkcode patchImage} edits Ventoy's boot image (`ventoy.disk.img`) before
 *   Ventoy writes it: the chainloader replaces GRUB (`grubx64_real.efi`), so it
 *   survives Ventoy's Update and both of its Secure Boot support modes.
 * @module
 */
import type { Fat } from "./fat.ts";

/**
 * Fixed locations on the EFI partition; must match the chainloader's constants.
 * `original` must start with `/EFI/BOOT/BOOT`: Ventoy's shim only takes its
 * `fbx64.efi` route to GRUB when its own path does.
 */
export const PATHS = {
  boot: "/EFI/BOOT/BOOTX64.EFI",
  original: "/EFI/BOOT/BOOTX64_ORIG.EFI",
  driverDir: "/EFI/claw",
  driver: "/EFI/claw/claw-dpad.efi",
  /** Ventoy's GRUB; shim reaches it through `fbx64.efi`. */
  grub: "/EFI/BOOT/grubx64_real.efi",
  /** Where {@linkcode patchImage} moves GRUB; the chainloader prefers it. */
  movedGrub: "/EFI/claw/grubx64.efi",
} as const;

/** Text inside every claw-chainload build; tells it apart from Ventoy's loader. */
const MARKER = new TextEncoder().encode("claw-chainload:");

/** What is on the partition now. */
export interface Status {
  /** The partition has Ventoy's files. */
  ventoy: boolean;
  /** `BOOTX64.EFI` is the chainloader. */
  installed: boolean;
  /** The renamed original loader is present. */
  original: boolean;
  /** The d-pad driver is present. */
  driver: boolean;
  /** The stick was made from a patched Ventoy package ({@linkcode patchImage}). */
  patched: boolean;
}

/** The files to install. */
export interface Payload {
  /** claw-chainload.efi */
  loader: Uint8Array;
  /** claw-dpad.efi */
  driver: Uint8Array;
}

/** Whether `needle` occurs in `hay`. */
function contains(hay: Uint8Array, needle: Uint8Array): boolean {
  outer: for (let i = 0; i <= hay.length - needle.length; i++) {
    for (let j = 0; j < needle.length; j++) if (hay[i + j] !== needle[j]) continue outer;
    return true;
  }
  return false;
}

/** Whether two byte arrays are equal. */
function same(a: Uint8Array, b: Uint8Array): boolean {
  return a.length === b.length && a.every((x, i) => x === b[i]);
}

/** Whether an EFI image is a claw-chainload build. */
export function isChainloader(image: Uint8Array): boolean {
  return contains(image, MARKER);
}

/** Writes a file and reads it back, so a bad stick fails before anything depends on it. */
async function writeVerified(fat: Fat, path: string, data: Uint8Array): Promise<void> {
  await fat.write(path, data);
  if (!same(await fat.read(path), data)) throw new Error(`verify failed: ${path}`);
}

/** Reports what is installed. */
export async function status(fat: Fat): Promise<Status> {
  const hasBoot = await fat.exists(PATHS.boot);
  return {
    ventoy: hasBoot && ((await fat.exists("/grub/grub.cfg")) || (await fat.exists("/ventoy"))),
    installed: hasBoot && isChainloader(await fat.read(PATHS.boot)),
    original: await fat.exists(PATHS.original),
    driver: await fat.exists(PATHS.driver),
    patched: await fat.exists(PATHS.movedGrub),
  };
}

/**
 * Installs or updates the chainloader and driver.
 *
 * When `BOOTX64.EFI` is not ours (a fresh stick, or Ventoy rewrote it during
 * an update), it becomes the new original: `backup` saves a host copy first,
 * then it is copied to `BOOTX64_ORIG.EFI` and verified, and only then is
 * `BOOTX64.EFI` overwritten.
 *
 * @param backup saves the current loader off the stick before it is touched
 * @param force install even if the partition doesn't look like Ventoy
 */
export async function install(
  fat: Fat,
  payload: Payload,
  backup: (loader: Uint8Array) => Promise<void>,
  force = false,
): Promise<void> {
  const now = await status(fat);
  if (!now.ventoy && !force) {
    throw new Error(`${fat.where} doesn't look like Ventoy's EFI partition (use --force)`);
  }
  if (now.patched) {
    throw new Error("this stick came from a patched Ventoy package and already has d-pad support");
  }
  if (!isChainloader(payload.loader)) throw new Error("the loader file is not claw-chainload");
  await writeVerified(fat, PATHS.driver, payload.driver);
  if (!now.installed) {
    const current = await fat.read(PATHS.boot);
    await backup(current);
    await writeVerified(fat, PATHS.original, current);
  } else if (!now.original) {
    throw new Error(
      `chainloader present but ${PATHS.original} is missing; restore it from a backup`,
    );
  }
  await writeVerified(fat, PATHS.boot, payload.loader);
}

/** Puts the original loader back and removes the driver. A no-op on a clean stick. */
export async function uninstall(fat: Fat): Promise<void> {
  const now = await status(fat);
  if (now.installed) {
    if (!now.original) {
      throw new Error(`${PATHS.original} is missing; restore BOOTX64.EFI from a backup`);
    }
    await writeVerified(fat, PATHS.boot, await fat.read(PATHS.original));
  }
  if (await fat.exists(PATHS.original)) await fat.remove(PATHS.original);
  if (now.driver) await fat.remove(PATHS.driver);
  if (await fat.exists(PATHS.driverDir)) await fat.removeDir(PATHS.driverDir);
}

/**
 * Adds the chainloader and driver to Ventoy's boot image (a VTOYEFI file
 * system, before Ventoy writes it to a stick). GRUB moves to
 * {@linkcode PATHS.movedGrub} and the chainloader takes its place, so:
 *
 * - Secure Boot support on: firmware → shim → `fbx64.efi` → chainloader → GRUB.
 * - Secure Boot support off: Ventoy copies `grubx64_real.efi` (now the
 *   chainloader) to `BOOTX64.EFI`, so firmware → chainloader → GRUB.
 *
 * @throws if the image isn't Ventoy's or is already patched
 */
export async function patchImage(fat: Fat, payload: Payload): Promise<void> {
  if (!(await fat.exists("/grub/grub.cfg")) || !(await fat.exists(PATHS.grub))) {
    throw new Error(`${fat.where} doesn't look like Ventoy's boot image`);
  }
  if (!isChainloader(payload.loader)) throw new Error("the loader file is not claw-chainload");
  const grub = await fat.read(PATHS.grub);
  if (isChainloader(grub) || (await fat.exists(PATHS.movedGrub))) {
    throw new Error(`${fat.where} is already patched`);
  }
  await writeVerified(fat, PATHS.driver, payload.driver);
  await writeVerified(fat, PATHS.movedGrub, grub);
  await writeVerified(fat, PATHS.grub, payload.loader);
}
