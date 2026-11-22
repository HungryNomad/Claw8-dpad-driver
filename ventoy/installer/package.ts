/**
 * claw-ventoy package: makes patched Ventoy release packages with MSI Claw
 * d-pad support built into the boot image.
 *
 *     deno task package ventoy-1.1.18-windows.zip ventoy-1.1.18-linux.tar.gz
 *
 * See README.md for options.
 * @module
 */
import { parseArgs } from "@std/cli/parse-args";
import { payloadFile } from "./main.ts";
import { patchPackage, versionOf } from "./src/package.ts";

const USAGE = `Usage: deno task package [options] <ventoy package>...

  --out <dir>       where the patched packages go (default: ./out)
  --loader <file>   claw-chainload.efi (default: dist/ or the cargo build output)
  --driver <file>   claw-dpad.efi (same default)
  --license <file>  Ventoy's license (its COPYING file), added as LICENSE-Ventoy.txt
  --source <url>    link to the exact d-pad sources, for the notice`;

/** Default link to the d-pad sources. */
const REPO = "https://github.com/HungryNomad/Claw8-dpad-driver";

/** Name of the notice file added to each package. */
export const NOTICE_NAME = "CLAW-DPAD-README.txt";

/** Name Ventoy's license gets in each package. */
export const LICENSE_NAME = "LICENSE-Ventoy.txt";

/**
 * The notice that goes into each package: what changed, and where the sources are.
 * @param license whether Ventoy's license is added as {@linkcode LICENSE_NAME}
 */
export function notice(version: string, source: string, license: boolean): string {
  const added = license
    ? `${NOTICE_NAME} (this file) and ${LICENSE_NAME} were`
    : `${NOTICE_NAME} (this file) was`;
  return `Ventoy ${version} with MSI Claw d-pad support
=============================================

This is Ventoy ${version}, unchanged except for its boot image
(ventoy/ventoy.disk.img.xz), which Ventoy writes to the stick's VTOYEFI
partition. In that image:

- /EFI/BOOT/grubx64_real.efi (Ventoy's GRUB) moved to /EFI/claw/grubx64.efi.
- /EFI/BOOT/grubx64_real.efi is now claw-chainload, which starts the d-pad
  driver and then Ventoy's GRUB.
- /EFI/claw/claw-dpad.efi was added: the d-pad driver.
- ${added} added to the package.

Secure Boot must be off in the firmware: the two added files are unsigned.
Ventoy's Install and Update both keep the d-pad support, with Ventoy's Secure
Boot support option on or off.

This is not an official Ventoy release; report d-pad problems to the
Claw8-dpad-driver project, not to Ventoy.

Sources
- Ventoy ${version}, licensed GPLv3${license ? ` (${LICENSE_NAME})` : ""}:
  https://github.com/ventoy/Ventoy/tree/v${version}
- claw-chainload and claw-dpad (MIT or Apache-2.0):
  ${source}
`;
}

/** Parses the command line and patches each package. */
async function main(args: string[]): Promise<void> {
  const opts = parseArgs(args, {
    string: ["out", "loader", "driver", "license", "source"],
    boolean: ["help"],
    default: { out: "out", source: REPO },
  });
  const packages = opts._.map(String);
  if (opts.help || !packages.length) {
    console.log(USAGE);
    if (!opts.help) Deno.exit(2);
    return;
  }
  const payload = {
    loader: await payloadFile("claw-chainload.efi", opts.loader),
    driver: await payloadFile("claw-dpad.efi", opts.driver),
  };
  const extra = opts.license ? { [LICENSE_NAME]: await Deno.readFile(opts.license) } : undefined;
  for (const pkg of packages) {
    const version = await versionOf(pkg);
    const out = await patchPackage(pkg, opts.out, {
      payload,
      notice: notice(version, opts.source, extra !== undefined),
      noticeName: NOTICE_NAME,
      extra,
    });
    console.log(`Ventoy ${version}: ${out}`);
  }
}

if (import.meta.main) {
  main(Deno.args).catch((e) => {
    console.error(`Error: ${e instanceof Error ? e.message : e}`);
    Deno.exit(1);
  });
}
