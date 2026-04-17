import * as fs from 'fs/promises';
import * as os from 'os';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { DraftStorage } from '../../src/utils/draft-storage';

vi.mock('fs/promises');
vi.mock('os');

describe('DraftStorage', () => {
  let storage: DraftStorage;
  const storageDir = '/home/user/.slack-cli/drafts';
  const fixedNow = new Date('2026-04-17T07:30:00.000Z').getTime();

  beforeEach(() => {
    vi.resetAllMocks();
    vi.mocked(os.homedir).mockReturnValue('/home/user');
    vi.useFakeTimers();
    vi.setSystemTime(fixedNow);
    storage = new DraftStorage();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  describe('save', () => {
    it('creates a new draft with generated id, createdAt and updatedAt', async () => {
      vi.mocked(fs.mkdir).mockResolvedValue(undefined);
      vi.mocked(fs.writeFile).mockResolvedValue();
      vi.mocked(fs.rename).mockResolvedValue();

      const result = await storage.save({
        channel: 'C0ARY9ESLCX',
        message: 'お疲れさまです！',
        profile: 'default',
      });

      expect(result.id).toMatch(/^draft_\d+_[0-9a-f]{6}$/);
      expect(result.channel).toBe('C0ARY9ESLCX');
      expect(result.message).toBe('お疲れさまです！');
      expect(result.profile).toBe('default');
      expect(result.createdAt).toBe('2026-04-17T07:30:00.000Z');
      expect(result.updatedAt).toBe('2026-04-17T07:30:00.000Z');

      expect(fs.mkdir).toHaveBeenCalledWith(
        storageDir,
        expect.objectContaining({ recursive: true })
      );
      expect(fs.writeFile).toHaveBeenCalledWith(
        expect.stringContaining(`${storageDir}/${result.id}.json`),
        expect.stringContaining('"channel": "C0ARY9ESLCX"'),
        expect.objectContaining({ mode: 0o600 })
      );
    });

    it('persists optional fields: thread, blocks, note, channelLabel', async () => {
      vi.mocked(fs.mkdir).mockResolvedValue(undefined);
      vi.mocked(fs.writeFile).mockResolvedValue();
      vi.mocked(fs.rename).mockResolvedValue();

      const blocks = [{ type: 'section', text: { type: 'mrkdwn', text: 'hi' } }];
      const result = await storage.save({
        channel: 'C123',
        channelLabel: '#dev-acejob',
        thread: '1700000000.000100',
        message: 'body',
        blocks,
        note: '田中さんへの返信',
        profile: 'work',
      });

      expect(result.thread).toBe('1700000000.000100');
      expect(result.blocks).toEqual(blocks);
      expect(result.note).toBe('田中さんへの返信');
      expect(result.channelLabel).toBe('#dev-acejob');
    });

    it('updates existing draft when id is provided', async () => {
      const existing = {
        id: 'draft_1700000000000_abcdef',
        channel: 'C123',
        message: 'old body',
        profile: 'default',
        createdAt: '2026-04-10T00:00:00.000Z',
        updatedAt: '2026-04-10T00:00:00.000Z',
      };
      vi.mocked(fs.readFile).mockResolvedValueOnce(JSON.stringify(existing));
      vi.mocked(fs.mkdir).mockResolvedValue(undefined);
      vi.mocked(fs.writeFile).mockResolvedValue();
      vi.mocked(fs.rename).mockResolvedValue();

      const result = await storage.save({
        id: existing.id,
        channel: 'C123',
        message: 'new body',
        profile: 'default',
      });

      expect(result.id).toBe(existing.id);
      expect(result.message).toBe('new body');
      expect(result.createdAt).toBe(existing.createdAt);
      expect(result.updatedAt).toBe('2026-04-17T07:30:00.000Z');
    });

    it('throws when updating a draft that does not exist', async () => {
      vi.mocked(fs.readFile).mockRejectedValueOnce(
        Object.assign(new Error('missing'), { code: 'ENOENT' })
      );

      await expect(
        storage.save({
          id: 'draft_1700000000000_abcdef',
          channel: 'C123',
          message: 'body',
          profile: 'default',
        })
      ).rejects.toThrow(/not found/i);
    });
  });

  describe('list', () => {
    it('returns drafts sorted by updatedAt descending', async () => {
      vi.mocked(fs.readdir).mockResolvedValue([
        'draft_1.json',
        'draft_2.json',
        'unrelated.txt',
      ] as unknown as never);

      const d1 = {
        id: 'draft_1',
        channel: 'C1',
        message: 'older',
        profile: 'default',
        createdAt: '2026-04-15T00:00:00.000Z',
        updatedAt: '2026-04-15T00:00:00.000Z',
      };
      const d2 = {
        id: 'draft_2',
        channel: 'C2',
        message: 'newer',
        profile: 'default',
        createdAt: '2026-04-17T00:00:00.000Z',
        updatedAt: '2026-04-17T00:00:00.000Z',
      };
      vi.mocked(fs.readFile).mockImplementation(async (p) => {
        const s = String(p);
        if (s.endsWith('draft_1.json')) return JSON.stringify(d1);
        if (s.endsWith('draft_2.json')) return JSON.stringify(d2);
        throw new Error(`unexpected ${s}`);
      });

      const drafts = await storage.list();
      expect(drafts.map((d) => d.id)).toEqual(['draft_2', 'draft_1']);
    });

    it('returns an empty array when no directory exists', async () => {
      vi.mocked(fs.readdir).mockRejectedValue(
        Object.assign(new Error('missing'), { code: 'ENOENT' })
      );

      const drafts = await storage.list();
      expect(drafts).toEqual([]);
    });

    it('skips unreadable/invalid files silently', async () => {
      vi.mocked(fs.readdir).mockResolvedValue([
        'draft_good.json',
        'draft_broken.json',
      ] as unknown as never);
      const good = {
        id: 'draft_good',
        channel: 'C1',
        message: 'ok',
        profile: 'default',
        createdAt: '2026-04-17T00:00:00.000Z',
        updatedAt: '2026-04-17T00:00:00.000Z',
      };
      vi.mocked(fs.readFile).mockImplementation(async (p) => {
        if (String(p).endsWith('draft_good.json')) return JSON.stringify(good);
        return 'not-json{{';
      });

      const drafts = await storage.list();
      expect(drafts).toHaveLength(1);
      expect(drafts[0].id).toBe('draft_good');
    });
  });

  describe('get', () => {
    it('returns a draft by id', async () => {
      const d = {
        id: 'draft_xyz',
        channel: 'C1',
        message: 'hi',
        profile: 'default',
        createdAt: '2026-04-17T00:00:00.000Z',
        updatedAt: '2026-04-17T00:00:00.000Z',
      };
      vi.mocked(fs.readFile).mockResolvedValueOnce(JSON.stringify(d));

      const result = await storage.get('draft_xyz');
      expect(result).toEqual(d);
    });

    it('returns null when draft does not exist', async () => {
      vi.mocked(fs.readFile).mockRejectedValueOnce(
        Object.assign(new Error('missing'), { code: 'ENOENT' })
      );

      const result = await storage.get('draft_missing');
      expect(result).toBeNull();
    });

    it('rejects path traversal attempts in id', async () => {
      await expect(storage.get('../etc/passwd')).rejects.toThrow(/invalid.*id/i);
    });
  });

  describe('delete', () => {
    it('removes the draft file', async () => {
      vi.mocked(fs.unlink).mockResolvedValue();

      await storage.delete('draft_xyz');

      expect(fs.unlink).toHaveBeenCalledWith(`${storageDir}/draft_xyz.json`);
    });

    it('throws when the draft does not exist', async () => {
      vi.mocked(fs.unlink).mockRejectedValue(
        Object.assign(new Error('missing'), { code: 'ENOENT' })
      );

      await expect(storage.delete('draft_missing')).rejects.toThrow(/not found/i);
    });

    it('rejects path traversal attempts in id', async () => {
      await expect(storage.delete('../etc/passwd')).rejects.toThrow(/invalid.*id/i);
    });
  });
});
