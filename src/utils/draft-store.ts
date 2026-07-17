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
      id: this.generateId(drafts),
      ...input,
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
      return Array.isArray(parsed) ? parsed : [];
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code === 'ENOENT') {
        return [];
      }
      throw error;
    }
  }

  private async writeDrafts(drafts: Draft[]): Promise<void> {
    await fs.mkdir(this.configDir, { recursive: true, mode: FILE_PERMISSIONS.CONFIG_DIR });
    await fs.writeFile(this.draftsPath, JSON.stringify(drafts, null, 2), {
      mode: FILE_PERMISSIONS.CONFIG_FILE,
    });
  }
}
