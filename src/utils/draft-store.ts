import { randomBytes } from 'crypto';
import * as fs from 'fs/promises';
import * as os from 'os';
import * as path from 'path';
import { FILE_PERMISSIONS } from './constants';
import { ValidationError } from './errors';

export interface Draft {
  id: string;
  channel?: string;
  user?: string;
  message: string;
  thread?: string;
  createdAt: string;
}

export interface DraftInput {
  channel?: string;
  user?: string;
  message: string;
  thread?: string;
}

export interface DraftStoreOptions {
  configDir?: string;
}

export class DraftStore {
  private draftsPath: string;
  private configDir: string;

  constructor(options: DraftStoreOptions = {}) {
    this.configDir = options.configDir || path.join(os.homedir(), '.slack-cli');
    this.draftsPath = path.join(this.configDir, 'drafts.json');
  }

  async save(input: DraftInput): Promise<Draft> {
    const drafts = await this.readDrafts();
    const draft: Draft = {
      ...input,
      id: this.generateId(drafts),
      createdAt: new Date().toISOString(),
    };
    drafts.push(draft);
    await this.writeDrafts(drafts);
    return draft;
  }

  async list(): Promise<Draft[]> {
    return await this.readDrafts();
  }

  async get(id: string): Promise<Draft | null> {
    const drafts = await this.readDrafts();
    return drafts.find((draft) => draft.id === id) ?? null;
  }

  async delete(id: string): Promise<void> {
    const drafts = await this.readDrafts();
    const remaining = drafts.filter((draft) => draft.id !== id);
    if (remaining.length === drafts.length) {
      throw new ValidationError(`Draft not found: ${id}`);
    }
    await this.writeDrafts(remaining);
  }

  private generateId(existing: Draft[]): string {
    let id: string;
    do {
      id = randomBytes(4).toString('hex');
    } while (existing.some((draft) => draft.id === id));
    return id;
  }

  private async readDrafts(): Promise<Draft[]> {
    try {
      const data = await fs.readFile(this.draftsPath, 'utf-8');
      const parsed = JSON.parse(data);
      if (!Array.isArray(parsed)) {
        return [];
      }
      return parsed.filter(
        (entry): entry is Draft =>
          typeof entry === 'object' &&
          entry !== null &&
          typeof entry.id === 'string' &&
          typeof entry.message === 'string'
      );
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code === 'ENOENT') {
        return [];
      }
      throw error;
    }
  }

  private async writeDrafts(drafts: Draft[]): Promise<void> {
    await fs.mkdir(this.configDir, { recursive: true, mode: FILE_PERMISSIONS.CONFIG_DIR });
    await fs.chmod(this.configDir, FILE_PERMISSIONS.CONFIG_DIR);

    // Write to a temp file and rename so a crash never leaves a half-written drafts.json
    const tempPath = `${this.draftsPath}.${process.pid}.${Date.now()}.tmp`;
    await fs.writeFile(tempPath, JSON.stringify(drafts, null, 2), {
      encoding: 'utf-8',
      mode: FILE_PERMISSIONS.CONFIG_FILE,
      flag: 'wx',
    });
    try {
      await fs.rename(tempPath, this.draftsPath);
    } catch (error) {
      await fs.unlink(tempPath).catch(() => undefined);
      throw error;
    }
  }
}
