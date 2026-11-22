/**
 * File access to a FAT partition, either through a mounted folder or through
 * mtools on an unmounted partition (a device or an image file).
 * @module
 */
import { dirname, join } from "@std/path";

/** Minimal file access to a FAT partition. Paths are absolute and `/`-separated. */
export interface Fat {
  /** Where the partition is, for messages. */
  readonly where: string;
  /** Whether a file or folder exists. */
  exists(path: string): Promise<boolean>;
  /** Reads a whole file. */
  read(path: string): Promise<Uint8Array>;
  /** Writes a whole file, creating its parent folders. */
  write(path: string, data: Uint8Array): Promise<void>;
  /** Deletes a file. */
  remove(path: string): Promise<void>;
  /** Deletes an empty folder. */
  removeDir(path: string): Promise<void>;
  /** Releases whatever was acquired to reach the partition (e.g. a drive letter). */
  close(): Promise<void>;
}

/** A partition mounted at a folder (a drive letter, `/media/...`, `/Volumes/...`). */
export class DirFat implements Fat {
  /**
   * @param root folder the partition is mounted at
   * @param onClose called once by {@linkcode DirFat.close}, e.g. to unmount
   */
  constructor(readonly root: string, private onClose?: () => Promise<void>) {}

  /** The mount folder. */
  get where(): string {
    return this.root;
  }

  /** Maps a partition path to a host path. */
  private host(path: string): string {
    return join(this.root, ...path.split("/").filter(Boolean));
  }

  /** Whether a file or folder exists. */
  async exists(path: string): Promise<boolean> {
    try {
      await Deno.stat(this.host(path));
      return true;
    } catch (e) {
      if (e instanceof Deno.errors.NotFound) return false;
      throw e;
    }
  }

  /** Reads a whole file. */
  read(path: string): Promise<Uint8Array> {
    return Deno.readFile(this.host(path));
  }

  /** Writes a whole file, creating its parent folders. */
  async write(path: string, data: Uint8Array): Promise<void> {
    const file = this.host(path);
    await Deno.mkdir(dirname(file), { recursive: true });
    await Deno.writeFile(file, data);
  }

  /** Deletes a file. */
  remove(path: string): Promise<void> {
    return Deno.remove(this.host(path));
  }

  /** Deletes an empty folder. */
  removeDir(path: string): Promise<void> {
    return Deno.remove(this.host(path));
  }

  /** Runs the close hook, once. */
  async close(): Promise<void> {
    const hook = this.onClose;
    this.onClose = undefined;
    await hook?.();
  }
}

/** An unmounted partition reached with mtools (`mcopy`, `mdir`, ...). */
export class MtoolsFat implements Fat {
  /** mtools image spec: `path@@offset`. */
  private readonly spec: string;

  /**
   * @param image disk, partition or image file
   * @param offset byte offset of the FAT file system inside `image`
   */
  constructor(readonly image: string, readonly offset = 0) {
    this.spec = `${image}@@${offset}`;
  }

  /** The image and offset. */
  get where(): string {
    return this.offset ? `${this.image} at byte ${this.offset}` : this.image;
  }

  /** Runs an mtools command on the partition; returns whether it succeeded. */
  private async run(cmd: string, ...args: string[]): Promise<{ ok: boolean; err: string }> {
    const out = await new Deno.Command(cmd, {
      args: ["-i", this.spec, ...args],
      env: { MTOOLS_SKIP_CHECK: "1" },
      stdout: "null",
      stderr: "piped",
    }).output();
    return { ok: out.success, err: new TextDecoder().decode(out.stderr).trim() };
  }

  /** Runs an mtools command and throws with its message if it fails. */
  private async must(cmd: string, ...args: string[]): Promise<void> {
    const { ok, err } = await this.run(cmd, ...args);
    if (!ok) throw new Error(`${cmd} ${args.join(" ")}: ${err || "failed"}`);
  }

  /** Whether a file or folder exists. */
  async exists(path: string): Promise<boolean> {
    return (await this.run("mdir", "-b", `::${path}`)).ok;
  }

  /** Reads a whole file through a temporary host file. */
  async read(path: string): Promise<Uint8Array> {
    const tmp = await Deno.makeTempFile();
    try {
      await this.must("mcopy", "-n", "-o", `::${path}`, tmp);
      return await Deno.readFile(tmp);
    } finally {
      await Deno.remove(tmp);
    }
  }

  /** Writes a whole file through a temporary host file, creating parent folders. */
  async write(path: string, data: Uint8Array): Promise<void> {
    const parts = path.split("/").filter(Boolean);
    for (let i = 1; i < parts.length; i++) {
      const dir = "/" + parts.slice(0, i).join("/");
      if (!(await this.exists(dir))) await this.must("mmd", `::${dir}`);
    }
    const tmp = await Deno.makeTempFile();
    try {
      await Deno.writeFile(tmp, data);
      await this.must("mcopy", "-o", tmp, `::${path}`);
    } finally {
      await Deno.remove(tmp);
    }
  }

  /** Deletes a file. */
  remove(path: string): Promise<void> {
    return this.must("mdel", `::${path}`);
  }

  /** Deletes an empty folder. */
  removeDir(path: string): Promise<void> {
    return this.must("mrd", `::${path}`);
  }

  /** Nothing to release. */
  close(): Promise<void> {
    return Promise.resolve();
  }
}

/** Whether mtools is on the PATH. */
export async function hasMtools(): Promise<boolean> {
  try {
    return (await new Deno.Command("mcopy", { args: ["-V"], stdout: "null", stderr: "null" })
      .output()).success;
  } catch {
    return false;
  }
}
