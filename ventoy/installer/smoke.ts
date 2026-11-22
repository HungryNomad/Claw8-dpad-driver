/**
 * claw-ventoy smoke test: boots a patched Ventoy package in QEMU with OVMF and
 * checks that Ventoy's menu comes up through the d-pad chainloader, once per
 * Ventoy Secure Boot support mode.
 *
 *     deno task smoke out/ventoy-1.1.18-claw-linux.tar.gz
 *
 * Needs qemu-system-x86_64, OVMF, mtools, mkfs.fat, xz and tar or unzip.
 * Screenshots of the last screen of each boot are saved as PNG (`--shots`).
 * @module
 */
import { parseArgs } from "@std/cli/parse-args";
import { join } from "@std/path";
import { MtoolsFat } from "./src/fat.ts";
import { findImage, kindOf } from "./src/package.ts";
import { isChainloader, PATHS } from "./src/stick.ts";

const USAGE = `Usage: deno task smoke [options] <patched ventoy package>

  --mode <on|off|both>  Ventoy Secure Boot support mode(s) to boot (default: both)
  --shots <dir>         where screenshots go (default: ./shots)
  --timeout <seconds>   per boot (default: 180)
  --ovmf <dir>          folder with OVMF_CODE_4M.fd and OVMF_VARS_4M.fd (default: /usr/share/OVMF)`;

const MiB = 1 << 20;
/** Ventoy's layout: partition 1 at 1 MiB, then the 32 MiB VTOYEFI partition. */
const PART1 = { start: MiB, size: 64 * MiB };
const PART2 = { start: PART1.start + PART1.size, size: 32 * MiB };
/** Ventoy's menu runs at this resolution; OVMF's own screens don't. */
const MENU = { width: 1024, height: 768 };
/** A themed menu has thousands of colours; firmware and GRUB text screens have a handful. */
const MIN_COLOURS = 1000;
/** Serial output that means the boot went wrong. */
const FAILURES = ["claw-chainload:", "WARNING", "error:"];

/** Terminal escape sequences, as OVMF writes them to serial. */
const ANSI = new RegExp(String.raw`\x1b\[[0-9;=?]*[a-zA-Z]`, "g");

/** Removes terminal escape sequences, so a serial log reads as text. */
function stripAnsi(log: string): string {
  return log.replace(ANSI, "");
}

/** Runs a command and returns stdout, throwing with stderr if it fails. */
async function run(cmd: string, args: string[]): Promise<Uint8Array> {
  const out = await new Deno.Command(cmd, { args, stdout: "piped", stderr: "piped" }).output();
  if (!out.success) throw new Error(`${cmd}: ${new TextDecoder().decode(out.stderr).trim()}`);
  return out.stdout;
}

/** Reads the files the test needs out of the package. */
async function unpack(
  pkg: string,
): Promise<{ boot: Uint8Array; core: Uint8Array; efi: Uint8Array }> {
  const kind = kindOf(pkg);
  const list = kind === "zip" ? ["-Z1", pkg] : ["tzf", pkg];
  const members = new TextDecoder().decode(await run(kind === "zip" ? "unzip" : "tar", list));
  const { member } = findImage(members.split("\n").filter(Boolean));
  const root = member.replace(/ventoy\/ventoy\.disk\.img\.xz$/, "");
  const get = (path: string) =>
    kind === "zip"
      ? run("unzip", ["-p", pkg, root + path])
      : run("tar", ["xzOf", pkg, root + path]);
  const unxz = async (data: Uint8Array) => {
    const tmp = await Deno.makeTempFile({ suffix: ".xz" });
    await Deno.writeFile(tmp, data);
    try {
      return await run("xz", ["-dc", tmp]);
    } finally {
      await Deno.remove(tmp);
    }
  };
  return {
    boot: await get("boot/boot.img"),
    core: await unxz(await get("boot/core.img.xz")),
    efi: await unxz(await get("ventoy/ventoy.disk.img.xz")),
  };
}

/** Returns a 16-byte MBR partition entry. */
function entry(boot: boolean, type: number, start: number, size: number): Uint8Array {
  const e = new Uint8Array(16);
  const v = new DataView(e.buffer);
  e.set([boot ? 0x80 : 0, 0xfe, 0xff, 0xff, type, 0xfe, 0xff, 0xff]);
  v.setUint32(8, start / 512, true);
  v.setUint32(12, size / 512, true);
  return e;
}

/**
 * Applies what Ventoy2Disk does to its image when Secure Boot support is off
 * (VentoyProcSecureBoot): GRUB, or whatever sits at `grubx64_real.efi`,
 * becomes `BOOTX64.EFI`, and shim and MokManager go.
 */
export async function secureBootSupportOff(fat: MtoolsFat): Promise<void> {
  const grub = await fat.read("/EFI/BOOT/grubx64_real.efi");
  for (
    const f of [
      "/EFI/BOOT/BOOTX64.EFI",
      "/EFI/BOOT/grubx64.efi",
      "/EFI/BOOT/grubx64_real.efi",
      "/EFI/BOOT/MokManager.efi",
      "/EFI/BOOT/mmx64.efi",
      "/ENROLL_THIS_KEY_IN_MOKMANAGER.cer",
      "/EFI/BOOT/grub.efi",
    ]
  ) if (await fat.exists(f)) await fat.remove(f);
  await fat.write("/EFI/BOOT/BOOTX64.EFI", grub);
}

/**
 * Writes a disk image laid out the way Ventoy2Disk installs to an MBR stick:
 * Ventoy's boot code and core.img, an empty FAT32 data partition labelled
 * Ventoy, and the VTOYEFI image as partition 2.
 */
async function stick(path: string, files: Awaited<ReturnType<typeof unpack>>, sbOff: boolean) {
  await Deno.writeFile(path, new Uint8Array(0));
  await Deno.truncate(path, PART2.start + PART2.size);
  await run("mkfs.fat", [
    "-F",
    "32",
    "-n",
    "Ventoy",
    "--offset",
    String(PART1.start / 512),
    path,
    String(PART1.size / 1024),
  ]);
  using f = await Deno.open(path, { write: true });
  const head = new Uint8Array(PART1.start);
  head.set(files.boot.subarray(0, 446));
  head.set(files.core.subarray(0, PART1.start - 512), 512);
  head.set([0x43, 0x4c, 0x41, 0x57], 440); // disk signature
  head.set(entry(true, 0x07, PART1.start, PART1.size), 446);
  head.set(entry(false, 0xef, PART2.start, PART2.size), 462);
  head.set([0x55, 0xaa], 510);
  await f.write(head);
  await f.seek(PART2.start, Deno.SeekMode.Start);
  await f.write(files.efi);
  const efi = new MtoolsFat(path, PART2.start);
  if (sbOff) await secureBootSupportOff(efi);
  // The chainloader must be what runs right before GRUB, or this proves nothing.
  if (!isChainloader(await efi.read(sbOff ? PATHS.boot : PATHS.grub))) {
    throw new Error("the package's boot image doesn't have the d-pad chainloader");
  }
}

/** A screen grab: size and RGB pixels. */
interface Screen {
  width: number;
  height: number;
  rgb: Uint8Array;
}

/** Parses a binary PPM (P6), as QEMU's screendump writes. */
function parsePpm(data: Uint8Array): Screen {
  const fields: string[] = [];
  let i = 0;
  while (fields.length < 4) {
    while (/\s/.test(String.fromCharCode(data[i]))) i++;
    let s = "";
    while (!/\s/.test(String.fromCharCode(data[i]))) s += String.fromCharCode(data[i++]);
    fields.push(s);
  }
  const [magic, w, h] = fields;
  if (magic !== "P6") throw new Error("not a P6 PPM");
  return { width: Number(w), height: Number(h), rgb: data.subarray(i + 1) };
}

/** Counts distinct colours, stopping early at `cap`. */
function colours(screen: Screen, cap: number): number {
  const seen = new Set<number>();
  const p = screen.rgb;
  for (let i = 0; i + 2 < p.length && seen.size < cap; i += 3) {
    seen.add((p[i] << 16) | (p[i + 1] << 8) | p[i + 2]);
  }
  return seen.size;
}

/** CRC-32 (PNG's checksum). */
function crc32(data: Uint8Array): number {
  let c = ~0;
  for (const b of data) {
    c ^= b;
    for (let k = 0; k < 8; k++) c = (c >>> 1) ^ (0xedb88320 & -(c & 1));
  }
  return ~c >>> 0;
}

/** Encodes a screen as PNG, so CI artifacts open anywhere. */
async function png(screen: Screen): Promise<Uint8Array> {
  const { width, height, rgb } = screen;
  const raw = new Uint8Array(height * (width * 3 + 1));
  for (let y = 0; y < height; y++) {
    raw.set(rgb.subarray(y * width * 3, (y + 1) * width * 3), y * (width * 3 + 1) + 1);
  }
  const zlib = new Uint8Array(
    await new Response(new Blob([raw]).stream().pipeThrough(new CompressionStream("deflate")))
      .arrayBuffer(),
  );
  const chunk = (type: string, body: Uint8Array) => {
    const out = new Uint8Array(body.length + 12);
    const v = new DataView(out.buffer);
    v.setUint32(0, body.length);
    out.set(new TextEncoder().encode(type), 4);
    out.set(body, 8);
    v.setUint32(body.length + 8, crc32(out.subarray(4, body.length + 8)));
    return out;
  };
  const ihdr = new Uint8Array(13);
  new DataView(ihdr.buffer).setUint32(0, width);
  new DataView(ihdr.buffer).setUint32(4, height);
  ihdr.set([8, 2, 0, 0, 0], 8);
  const parts = [
    new Uint8Array([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk("IHDR", ihdr),
    chunk("IDAT", zlib),
    chunk("IEND", new Uint8Array()),
  ];
  return new Uint8Array(await new Blob(parts).arrayBuffer());
}

/** Sends one command to QEMU's human monitor on a unix socket. */
async function monitor(socket: string, command: string): Promise<void> {
  using conn = await Deno.connect({ transport: "unix", path: socket });
  await conn.write(new TextEncoder().encode(`${command}\n`));
  await new Promise((r) => setTimeout(r, 500));
}

/** Boot settings shared by every run. */
interface BootOptions {
  ovmf: string;
  timeout: number;
  shots: string;
}

/**
 * Boots `disk` and waits for Ventoy's menu. Returns whether it appeared;
 * saves the last screen as `<shots>/<name>.png` either way.
 */
async function boot(disk: string, name: string, opts: BootOptions): Promise<boolean> {
  const work = await Deno.makeTempDir({ prefix: "claw-smoke-" });
  const [vars, socket, serial, dump] = ["vars.fd", "mon.sock", "serial.log", "screen.ppm"]
    .map((f) => join(work, f));
  await Deno.copyFile(join(opts.ovmf, "OVMF_VARS_4M.fd"), vars);
  const qemu = new Deno.Command("qemu-system-x86_64", {
    args: [
      ...["-machine", "q35", "-m", "1024", "-accel", "kvm", "-accel", "tcg"],
      ...["-display", "none", "-vga", "std", "-net", "none"],
      "-drive",
      `if=pflash,format=raw,readonly=on,file=${join(opts.ovmf, "OVMF_CODE_4M.fd")}`,
      ...["-drive", `if=pflash,format=raw,file=${vars}`],
      ...["-device", "qemu-xhci", "-drive", `if=none,id=stick,format=raw,file=${disk}`],
      ...["-device", "usb-storage,drive=stick,bootindex=1"],
      ...["-monitor", `unix:${socket},server,nowait`, "-serial", `file:${serial}`],
    ],
    stdout: "null",
    stderr: "piped",
  }).spawn();
  let screen: Screen | undefined;
  let ok = false;
  let why = "timed out";
  try {
    const deadline = Date.now() + opts.timeout * 1000;
    while (Date.now() < deadline) {
      await new Promise((r) => setTimeout(r, 3000));
      const log = await Deno.readTextFile(serial).catch(() => "");
      const failure = FAILURES.find((f) => log.includes(f));
      if (failure) {
        why = `serial output has "${failure}"`;
        break;
      }
      await monitor(socket, `screendump ${dump}`).catch(() => {});
      screen = await Deno.readFile(dump).then(parsePpm).catch(() => screen);
      if (
        screen?.width === MENU.width && screen.height === MENU.height &&
        colours(screen, MIN_COLOURS) >= MIN_COLOURS
      ) {
        ok = true;
        break;
      }
    }
    if (!ok) {
      const log = await Deno.readTextFile(serial).catch(() => "");
      console.error(
        `${name}: ${why}. Serial output:\n${stripAnsi(log)}`,
      );
    }
  } finally {
    qemu.kill();
    await qemu.status;
    if (screen) {
      await Deno.mkdir(opts.shots, { recursive: true });
      await Deno.writeFile(join(opts.shots, `${name}.png`), await png(screen));
    }
    await Deno.remove(work, { recursive: true });
  }
  return ok;
}

/** Parses the command line and boots each requested mode. */
async function main(args: string[]): Promise<void> {
  const opts = parseArgs(args, {
    string: ["mode", "shots", "timeout", "ovmf"],
    boolean: ["help"],
    default: { mode: "both", shots: "shots", timeout: "180", ovmf: "/usr/share/OVMF" },
  });
  const [pkg] = opts._.map(String);
  const modes = { on: ["on"], off: ["off"], both: ["on", "off"] }[opts.mode];
  if (opts.help || !pkg || !modes) {
    console.log(USAGE);
    if (!opts.help) Deno.exit(2);
    return;
  }
  const files = await unpack(pkg);
  const bootOpts = { ovmf: opts.ovmf, timeout: Number(opts.timeout), shots: opts.shots };
  let failed = 0;
  for (const mode of modes) {
    const disk = await Deno.makeTempFile({ suffix: ".img" });
    try {
      await stick(disk, files, mode === "off");
      const ok = await boot(disk, `secure-boot-support-${mode}`, bootOpts);
      console.log(`Secure Boot support ${mode}: ${ok ? "Ventoy menu appeared" : "FAILED"}`);
      if (!ok) failed++;
    } finally {
      await Deno.remove(disk);
    }
  }
  if (failed) Deno.exit(1);
}

if (import.meta.main) {
  main(Deno.args).catch((e) => {
    console.error(`Error: ${e instanceof Error ? e.message : e}`);
    Deno.exit(1);
  });
}
