/**
 * claw-ventoy: adds MSI Claw d-pad support to a Ventoy stick (and removes it).
 *
 *     deno task install     # or: uninstall, status
 *
 * See README.md for options.
 * @module
 */
import { parseArgs } from "@std/cli/parse-args";
import { join } from "@std/path";
import { open } from "./src/locate.ts";
import { install, status, uninstall } from "./src/stick.ts";

const USAGE = `Usage: deno task <install|uninstall|status> [options]

  --efi <dir>       Ventoy's EFI partition (VTOYEFI) is mounted at <dir>
  --image <path>    disk, partition or image file to edit with mtools
  --offset <bytes>  byte offset of VTOYEFI in --image (default: from the partition table)
  --loader <file>   claw-chainload.efi (default: dist/ or the cargo build output)
  --driver <file>   claw-dpad.efi (same default)
  --backup <dir>    where the stick's original BOOTX64.EFI is saved (default: ./backup)
  --force           install even if the partition doesn't look like Ventoy`;

/** Folders searched for the .efi files when no path is given. */
const SEARCH = [
  join(import.meta.dirname!, "dist"),
  join(import.meta.dirname!, "..", "..", "target", "x86_64-unknown-uefi", "release"),
];

/** Reads `explicit`, or the first `name` found in {@linkcode SEARCH}. */
async function payloadFile(name: string, explicit?: string): Promise<Uint8Array> {
  for (const path of explicit ? [explicit] : SEARCH.map((d) => join(d, name))) {
    try {
      return await Deno.readFile(path);
    } catch (e) {
      if (explicit || !(e instanceof Deno.errors.NotFound)) throw e;
    }
  }
  throw new Error(`${name} not found; build it (cargo efi) or pass its path`);
}

/** Returns a backup function that saves a loader as `<dir>/BOOTX64-<time>.EFI`. */
function backupTo(dir: string): (loader: Uint8Array) => Promise<void> {
  return async (loader) => {
    await Deno.mkdir(dir, { recursive: true });
    const file = join(dir, `BOOTX64-${new Date().toISOString().replace(/[:.]/g, "-")}.EFI`);
    await Deno.writeFile(file, loader);
    console.log(`Saved the stick's original loader to ${file}`);
  };
}

/** Parses the command line and runs the command. */
async function main(args: string[]): Promise<void> {
  const opts = parseArgs(args, {
    string: ["efi", "image", "offset", "loader", "driver", "backup"],
    boolean: ["force", "help"],
    default: { backup: "backup" },
  });
  const [cmd] = opts._.map(String);
  if (opts.help || !["install", "uninstall", "status"].includes(cmd)) {
    console.log(USAGE);
    if (!opts.help) Deno.exit(2);
    return;
  }
  const offset = opts.offset === undefined ? undefined : Number(opts.offset);
  const fat = await open({ efi: opts.efi, image: opts.image, offset });
  try {
    console.log(`Ventoy EFI partition: ${fat.where}`);
    if (cmd === "install") {
      const loader = await payloadFile("claw-chainload.efi", opts.loader);
      const driver = await payloadFile("claw-dpad.efi", opts.driver);
      await install(fat, { loader, driver }, backupTo(opts.backup), opts.force);
      console.log("Installed. Secure Boot must be off to boot this stick in UEFI mode now.");
    } else if (cmd === "uninstall") {
      await uninstall(fat);
      console.log("Removed. The stick boots exactly as Ventoy set it up.");
    } else {
      const s = await status(fat);
      console.log(
        s.installed
          ? `D-pad support is installed${s.driver ? "" : ", but the driver is missing"}.`
          : `D-pad support is not installed${
            s.ventoy ? "" : " (and this doesn't look like Ventoy)"
          }.`,
      );
    }
  } finally {
    await fat.close();
  }
}

if (import.meta.main) {
  main(Deno.args).catch((e) => {
    console.error(`Error: ${e instanceof Error ? e.message : e}`);
    Deno.exit(1);
  });
}
