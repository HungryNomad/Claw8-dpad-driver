/**
 * Finds Ventoy's EFI partition (FAT label `VTOYEFI`) inside a disk, partition
 * or image file by reading its MBR or GPT partition table.
 * @module
 */

/** Volume label Ventoy gives its EFI partition. */
export const EFI_LABEL = "VTOYEFI";

const SECTOR = 512;

/** A partition's location on disk, in bytes. */
export interface Partition {
  /** 1-based partition number. */
  number: number;
  /** Byte offset from the start of the disk. */
  offset: number;
  /** Size in bytes. */
  size: number;
}

/** Reads `length` bytes at `offset`, in whole sectors (raw devices need that). */
export type ReadAt = (offset: number, length: number) => Promise<Uint8Array>;

/** Returns a {@linkcode ReadAt} over an open file. Short reads are zero-padded. */
export function fileReader(file: Deno.FsFile): ReadAt {
  return async (offset, length) => {
    const buf = new Uint8Array(Math.ceil(length / SECTOR) * SECTOR);
    await file.seek(offset, Deno.SeekMode.Start);
    for (let got = 0; got < buf.length;) {
      const n = await file.read(buf.subarray(got));
      if (n === null) break;
      got += n;
    }
    return buf.subarray(0, length);
  };
}

/** Returns the volume label of a FAT boot sector, or `undefined` if it isn't one. */
export function fatLabel(boot: Uint8Array): string | undefined {
  if (boot.length < SECTOR || boot[510] !== 0x55 || boot[511] !== 0xaa) return undefined;
  const view = new DataView(boot.buffer, boot.byteOffset, boot.byteLength);
  const fat32 = view.getUint16(0x16, true) === 0;
  const [sig, label] = fat32 ? [0x42, 0x47] : [0x26, 0x2b];
  if (boot[sig] !== 0x29) return undefined;
  return new TextDecoder().decode(boot.subarray(label, label + 11)).trimEnd();
}

/** Lists the partitions of a disk from its MBR, or from its GPT when the MBR is protective. */
export async function partitions(read: ReadAt): Promise<Partition[]> {
  const mbr = await read(0, SECTOR);
  if (mbr[510] !== 0x55 || mbr[511] !== 0xaa) return [];
  const view = new DataView(mbr.buffer, mbr.byteOffset);
  const entries = [0, 1, 2, 3].map((i) => 446 + i * 16);
  if (entries.some((e) => mbr[e + 4] === 0xee)) return gpt(read);
  return entries
    .map((e, i) => ({
      number: i + 1,
      type: mbr[e + 4],
      offset: view.getUint32(e + 8, true) * SECTOR,
      size: view.getUint32(e + 12, true) * SECTOR,
    }))
    .filter((p) => p.type !== 0 && p.size > 0)
    .map(({ number, offset, size }) => ({ number, offset, size }));
}

/** Lists the used entries of a GPT partition table. */
async function gpt(read: ReadAt): Promise<Partition[]> {
  const head = await read(SECTOR, SECTOR);
  if (new TextDecoder().decode(head.subarray(0, 8)) !== "EFI PART") return [];
  const hv = new DataView(head.buffer, head.byteOffset);
  const table = Number(hv.getBigUint64(72, true)) * SECTOR;
  const count = Math.min(hv.getUint32(80, true), 256);
  const size = hv.getUint32(84, true);
  const bytes = await read(table, count * size);
  const tv = new DataView(bytes.buffer, bytes.byteOffset);
  const out: Partition[] = [];
  for (let i = 0; i < count; i++) {
    const e = i * size;
    if (bytes.subarray(e, e + 16).every((b) => b === 0)) continue;
    const first = Number(tv.getBigUint64(e + 32, true));
    const last = Number(tv.getBigUint64(e + 40, true));
    out.push({ number: i + 1, offset: first * SECTOR, size: (last - first + 1) * SECTOR });
  }
  return out;
}

/**
 * Returns the byte offset of the `VTOYEFI` file system: 0 if `read` is that
 * partition itself, else the start of the matching partition.
 * @throws if no partition carries the label
 */
export async function findEfiOffset(read: ReadAt): Promise<number> {
  if (fatLabel(await read(0, SECTOR)) === EFI_LABEL) return 0;
  for (const p of await partitions(read)) {
    if (fatLabel(await read(p.offset, SECTOR)) === EFI_LABEL) return p.offset;
  }
  throw new Error(`no FAT partition labelled ${EFI_LABEL} found`);
}
