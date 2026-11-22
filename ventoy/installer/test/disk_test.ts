import { assertEquals, assertRejects } from "@std/assert";
import { fatLabel, findEfiOffset, partitions, type ReadAt } from "../src/disk.ts";

/** A FAT16 boot sector with the given label. */
function bootSector(label: string): Uint8Array {
  const b = new Uint8Array(512);
  new DataView(b.buffer).setUint16(0x16, 32, true); // sectors per FAT: FAT16
  b[0x26] = 0x29;
  b.set(new TextEncoder().encode(label.padEnd(11)), 0x2b);
  b[510] = 0x55;
  b[511] = 0xaa;
  return b;
}

/** A ReadAt over an in-memory disk. */
function mem(disk: Uint8Array): ReadAt {
  return (o, n) => {
    const out = new Uint8Array(n);
    out.set(disk.subarray(o, Math.min(o + n, disk.length)));
    return Promise.resolve(out);
  };
}

/** A disk with an MBR: partition 1 "Ventoy", partition 2 "VTOYEFI" at sector 64. */
function mbrDisk(): Uint8Array {
  const disk = new Uint8Array(70 * 512);
  const v = new DataView(disk.buffer);
  const entry = (i: number, type: number, lba: number, n: number) => {
    disk[446 + i * 16 + 4] = type;
    v.setUint32(446 + i * 16 + 8, lba, true);
    v.setUint32(446 + i * 16 + 12, n, true);
  };
  entry(0, 0x07, 32, 32);
  entry(1, 0xef, 64, 6);
  disk[510] = 0x55;
  disk[511] = 0xaa;
  disk.set(bootSector("Ventoy"), 32 * 512);
  disk.set(bootSector("VTOYEFI"), 64 * 512);
  return disk;
}

/** The same layout in a GPT. */
function gptDisk(): Uint8Array {
  const disk = new Uint8Array(80 * 512);
  const v = new DataView(disk.buffer);
  disk[446 + 4] = 0xee;
  disk[510] = 0x55;
  disk[511] = 0xaa;
  disk.set(new TextEncoder().encode("EFI PART"), 512);
  v.setBigUint64(512 + 72, 2n, true);
  v.setUint32(512 + 80, 128, true);
  v.setUint32(512 + 84, 128, true);
  const entry = (i: number, first: number, last: number) => {
    const e = 1024 + i * 128;
    disk[e] = 0xaa; // non-zero type GUID
    v.setBigUint64(e + 32, BigInt(first), true);
    v.setBigUint64(e + 40, BigInt(last), true);
  };
  entry(0, 40, 69);
  entry(1, 70, 79);
  disk.set(bootSector("Ventoy"), 40 * 512);
  disk.set(bootSector("VTOYEFI"), 70 * 512);
  return disk;
}

Deno.test("fatLabel reads FAT16 labels and rejects other sectors", () => {
  assertEquals(fatLabel(bootSector("VTOYEFI")), "VTOYEFI");
  assertEquals(fatLabel(new Uint8Array(512)), undefined);
});

Deno.test("partitions lists MBR entries", async () => {
  assertEquals(await partitions(mem(mbrDisk())), [
    { number: 1, offset: 32 * 512, size: 32 * 512 },
    { number: 2, offset: 64 * 512, size: 6 * 512 },
  ]);
});

Deno.test("findEfiOffset finds VTOYEFI in MBR, GPT and bare partition images", async () => {
  assertEquals(await findEfiOffset(mem(mbrDisk())), 64 * 512);
  assertEquals(await findEfiOffset(mem(gptDisk())), 70 * 512);
  assertEquals(await findEfiOffset(mem(bootSector("VTOYEFI"))), 0);
});

Deno.test("findEfiOffset fails without VTOYEFI", async () => {
  await assertRejects(() => findEfiOffset(mem(bootSector("OTHER"))));
});
