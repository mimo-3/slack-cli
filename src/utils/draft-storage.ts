import * as crypto from 'crypto';
import * as fs from 'fs/promises';
import * as os from 'os';
import * as path from 'path';
import { FILE_PERMISSIONS } from './constants';

export interface DraftRecord {
  id: string;
  channel: string;
  channelLabel?: string;
  thread?: string;
  message?: string;
  blocks?: unknown[];
  note?: string;
  profile: string;
  createdAt: string;
  updatedAt: string;
}

export interface DraftSaveInput {
  id?: string;
  channel: string;
  channelLabel?: string;
  thread?: string;
  message?: string;
  blocks?: unknown[];
  note?: string;
  profile: string;
}

export interface DraftStorageOptions {
  storageDir?: string;
}

const DRAFT_ID_PATTERN = /^[A-Za-z0-9_-]+$/;
const GENERATED_ID_SUFFIX_BYTES = 3;

export class DraftStorage {
  private storageDir: string;

  constructor(options: DraftStorageOptions = {}) {
    this.storageDir = options.storageDir || path.join(os.homedir(), '.slack-cli', 'drafts');
  }

  async save(input: DraftSaveInput): Promise<DraftRecord> {
    const nowIso = new Date().toISOString();

    let record: DraftRecord;
    if (input.id) {
      const existing = await this.get(input.id);
      if (!existing) {
        throw new Error(`Draft not found: ${input.id}`);
      }
      record = {
        ...existing,
        channel: input.channel,
        channelLabel: input.channelLabel,
        thread: input.thread,
        message: input.message,
        blocks: input.blocks,
        note: input.note,
        profile: input.profile,
        updatedAt: nowIso,
      };
    } else {
      const id = this.generateId();
      record = {
        id,
        channel: input.channel,
        channelLabel: input.channelLabel,
        thread: input.thread,
        message: input.message,
        blocks: input.blocks,
        note: input.note,
        profile: input.profile,
        createdAt: nowIso,
        updatedAt: nowIso,
      };
    }

    await this.writeRecord(record);
    return record;
  }

  async list(): Promise<DraftRecord[]> {
    let entries: string[];
    try {
      entries = (await fs.readdir(this.storageDir)) as unknown as string[];
    } catch (error: unknown) {
      if (isEnoent(error)) {
        return [];
      }
      throw error;
    }

    const jsonFiles = entries.filter((name) => name.endsWith('.json'));
    const records: DraftRecord[] = [];
    for (const name of jsonFiles) {
      const filePath = path.join(this.storageDir, name);
      try {
        const raw = await fs.readFile(filePath, 'utf-8');
        const parsed = JSON.parse(raw) as DraftRecord;
        records.push(parsed);
      } catch {
        // Silently skip unreadable or invalid draft files
      }
    }

    records.sort((a, b) => b.updatedAt.localeCompare(a.updatedAt));
    return records;
  }

  async get(id: string): Promise<DraftRecord | null> {
    this.assertValidId(id);
    const filePath = this.pathFor(id);
    try {
      const raw = await fs.readFile(filePath, 'utf-8');
      return JSON.parse(raw) as DraftRecord;
    } catch (error: unknown) {
      if (isEnoent(error)) {
        return null;
      }
      throw error;
    }
  }

  async delete(id: string): Promise<void> {
    this.assertValidId(id);
    try {
      await fs.unlink(this.pathFor(id));
    } catch (error: unknown) {
      if (isEnoent(error)) {
        throw new Error(`Draft not found: ${id}`);
      }
      throw error;
    }
  }

  private async writeRecord(record: DraftRecord): Promise<void> {
    await fs.mkdir(this.storageDir, {
      recursive: true,
      mode: FILE_PERMISSIONS.CONFIG_DIR,
    });

    const finalPath = this.pathFor(record.id);
    const tempPath = `${finalPath}.${process.pid}.${Date.now()}.tmp`;
    const payload = JSON.stringify(record, null, 2);

    await fs.writeFile(tempPath, payload, {
      encoding: 'utf-8',
      mode: FILE_PERMISSIONS.CONFIG_FILE,
      flag: 'w',
    });

    try {
      await fs.rename(tempPath, finalPath);
    } catch (error) {
      await fs.unlink(tempPath).catch(() => undefined);
      throw error;
    }
  }

  private pathFor(id: string): string {
    return path.join(this.storageDir, `${id}.json`);
  }

  private assertValidId(id: string): void {
    if (!DRAFT_ID_PATTERN.test(id)) {
      throw new Error(`Invalid draft id: ${id}`);
    }
  }

  private generateId(): string {
    const ts = Date.now();
    const rand = crypto.randomBytes(GENERATED_ID_SUFFIX_BYTES).toString('hex');
    return `draft_${ts}_${rand}`;
  }
}

function isEnoent(error: unknown): boolean {
  return (
    typeof error === 'object' &&
    error !== null &&
    'code' in error &&
    (error as { code?: string }).code === 'ENOENT'
  );
}
