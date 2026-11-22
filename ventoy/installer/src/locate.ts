/**
 * Finds the Ventoy stick's EFI partition and opens it as a {@linkcode Fat}:
 * a mounted folder when there is one, otherwise mtools on the raw partition.
 * @module
 */
import { DirFat, type Fat, hasMtools, MtoolsFat } from "./fat.ts";
import { EFI_LABEL, fileReader, findEfiOffset } from "./disk.ts";

/** Where to look, from the command line. All optional. */
export interface Where {
  /** Folder the partition is mounted at. */
  efi?: string;
  /** Disk, partition or image file, opened with mtools. */
  image?: string;
  /** Byte offset of the partition inside `image`; found from the partition table if absent. */
  offset?: number;
}

/** A partition as Windows reports it (see {@linkcode WIN_LIST}). */
export interface WinPart {
  disk: number;
  part: number;
  offset: number;
  size: number;
  letter: string;
  label: string | null;
}

/** PowerShell that lists every partition with its volume label as JSON. */
const WIN_LIST = `
$p = Get-Partition | ForEach-Object {
  $v = $_ | Get-Volume -ErrorAction SilentlyContinue
  [pscustomobject]@{ disk = $_.DiskNumber; part = $_.PartitionNumber; offset = $_.Offset;
    size = $_.Size; letter = [string]$_.DriveLetter; label = $v.FileSystemLabel }
}
ConvertTo-Json -Compress -InputObject @($p)`;

/** Runs a command and returns its stdout, throwing with stderr on failure. */
async function run(cmd: string, args: string[]): Promise<string> {
  const out = await new Deno.Command(cmd, { args, stdout: "piped", stderr: "piped" }).output();
  const text = (b: Uint8Array) => new TextDecoder().decode(b).trim();
  if (!out.success) throw new Error(`${cmd}: ${text(out.stderr) || "failed"}`);
  return text(out.stdout);
}

/** Runs a Windows PowerShell script. */
function powershell(script: string): Promise<string> {
  return run("powershell.exe", ["-NoProfile", "-NonInteractive", "-Command", script]);
}

/** Returns the one candidate, or throws naming what was (not) found. */
function one<T>(found: T[], what: string): T {
  if (found.length === 1) return found[0];
  throw new Error(
    found.length
      ? `found ${found.length} ${what}s; unplug the others or pass --efi or --image`
      : `no ${what} found; is the Ventoy stick plugged in? (or pass --efi or --image)`,
  );
}

/**
 * Picks Ventoy's EFI partition from Windows' partition list: the volume
 * labelled VTOYEFI, or else partition 2 of a disk whose partition 1 is
 * labelled "Ventoy" (Windows may not read the label of an EFI-type partition).
 */
export function pickWindows(parts: WinPart[]): WinPart {
  const labelled = parts.filter((p) => p.label === EFI_LABEL);
  if (labelled.length) return one(labelled, "Ventoy EFI partition");
  const ventoyDisks = new Set(
    parts.filter((p) => p.part === 1 && p.label === "Ventoy").map((p) => p.disk),
  );
  return one(parts.filter((p) => p.part === 2 && ventoyDisks.has(p.disk)), "Ventoy EFI partition");
}

/** Opens the partition on Windows: its drive letter, a temporary one, or mtools on the raw disk. */
async function openWindows(): Promise<Fat> {
  const parts = JSON.parse(await powershell(WIN_LIST)) as WinPart[];
  const p = pickWindows(parts);
  const letter = (l: string) => /^[A-Z]$/i.test(l) ? `${l}:\\` : undefined;
  const mounted = letter(p.letter);
  if (mounted) return new DirFat(mounted);
  const sel = `-DiskNumber ${p.disk} -PartitionNumber ${p.part}`;
  try {
    await powershell(`Add-PartitionAccessPath ${sel} -AssignDriveLetter`);
    const got = letter(await powershell(`[string](Get-Partition ${sel}).DriveLetter`));
    if (got) {
      return new DirFat(
        got,
        () => powershell(`Remove-PartitionAccessPath ${sel} -AccessPath '${got}'`).then(() => {}),
      );
    }
  } catch {
    // Windows refused a letter (common for EFI-type partitions); try mtools.
  }
  if (await hasMtools()) return new MtoolsFat(`\\\\.\\PhysicalDrive${p.disk}`, p.offset);
  throw new Error(
    "Windows wouldn't give the Ventoy EFI partition a drive letter and mtools isn't installed. " +
      "Run from an admin terminal, install mtools, or pass --efi <folder>.",
  );
}

/** A block device as lsblk reports it. */
interface LsblkDev {
  path: string;
  label: string | null;
  mountpoint: string | null;
  children?: LsblkDev[];
}

/** Opens the partition on Linux: its mount folder, or mtools on the partition device. */
async function openLinux(): Promise<Fat> {
  const { blockdevices } = JSON.parse(
    await run("lsblk", ["-J", "-o", "PATH,LABEL,MOUNTPOINT"]),
  ) as { blockdevices: LsblkDev[] };
  const flat = (d: LsblkDev): LsblkDev[] => [d, ...(d.children ?? []).flatMap(flat)];
  const dev = one(
    blockdevices.flatMap(flat).filter((d) => d.label === EFI_LABEL),
    "Ventoy EFI partition",
  );
  return dev.mountpoint ? new DirFat(dev.mountpoint) : new MtoolsFat(dev.path);
}

/** Opens the partition on macOS, where it must be mounted at /Volumes/VTOYEFI. */
async function openMac(): Promise<Fat> {
  const dir = `/Volumes/${EFI_LABEL}`;
  try {
    await Deno.stat(dir);
  } catch {
    throw new Error(`${dir} isn't mounted; mount it (diskutil mount) or pass --efi or --image`);
  }
  return new DirFat(dir);
}

/** Opens Ventoy's EFI partition from the options, or finds it on this computer. */
export async function open(where: Where): Promise<Fat> {
  if (where.efi) return new DirFat(where.efi);
  if (where.image) {
    let offset = where.offset;
    if (offset === undefined) {
      using file = await Deno.open(where.image);
      offset = await findEfiOffset(fileReader(file));
    }
    return new MtoolsFat(where.image, offset);
  }
  switch (Deno.build.os) {
    case "windows":
      return openWindows();
    case "linux":
      return openLinux();
    case "darwin":
      return openMac();
    default:
      throw new Error(`can't find the stick on ${Deno.build.os}; pass --efi or --image`);
  }
}
