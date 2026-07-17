import * as fs from 'fs/promises';
import * as os from 'os';
import * as path from 'path';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { DraftStore } from '../../src/utils/draft-store';

describe('DraftStore', () => {
  let tmpDir: string;
  let store: DraftStore;

  beforeEach(async () => {
    tmpDir = await fs.mkdtemp(path.join(os.tmpdir(), 'slack-cli-draft-test-'));
    store = new DraftStore({ configDir: tmpDir });
  });

  afterEach(async () => {
    await fs.rm(tmpDir, { recursive: true, force: true });
  });

  describe('save', () => {
    it('should save a draft and assign an id and createdAt', async () => {
      const draft = await store.save({ channel: 'general', message: 'hello' });

      expect(draft.id).toBeTruthy();
      expect(draft.channel).toBe('general');
      expect(draft.message).toBe('hello');
      expect(draft.createdAt).toBeTruthy();
    });

    it('should assign unique ids to each draft', async () => {
      const first = await store.save({ channel: 'general', message: 'one' });
      const second = await store.save({ channel: 'general', message: 'two' });

      expect(first.id).not.toBe(second.id);
    });

    it('should persist drafts to disk', async () => {
      await store.save({ channel: 'general', message: 'hello' });

      const anotherStore = new DraftStore({ configDir: tmpDir });
      const drafts = await anotherStore.list();

      expect(drafts).toHaveLength(1);
      expect(drafts[0].message).toBe('hello');
    });

    it('should save a draft targeting a user DM', async () => {
      const draft = await store.save({ user: 'alice', message: 'hi alice' });

      expect(draft.user).toBe('alice');
      expect(draft.channel).toBeUndefined();
    });

    it('should save a draft with thread timestamp', async () => {
      const draft = await store.save({
        channel: 'general',
        message: 'reply',
        thread: '1234567890.123456',
      });

      expect(draft.thread).toBe('1234567890.123456');
    });
  });

  describe('file safety', () => {
    it('should write drafts.json with owner-only permissions', async () => {
      await store.save({ channel: 'general', message: 'hello' });

      const stat = await fs.stat(path.join(tmpDir, 'drafts.json'));
      expect(stat.mode & 0o777).toBe(0o600);
    });

    it('should skip malformed entries in drafts.json', async () => {
      await fs.writeFile(
        path.join(tmpDir, 'drafts.json'),
        JSON.stringify([{ id: 'ok', message: 'valid' }, { broken: true }, 'garbage'])
      );

      const drafts = await store.list();

      expect(drafts).toHaveLength(1);
      expect(drafts[0].id).toBe('ok');
    });
  });

  describe('list', () => {
    it('should return empty array when no drafts exist', async () => {
      const drafts = await store.list();

      expect(drafts).toEqual([]);
    });

    it('should list all saved drafts', async () => {
      await store.save({ channel: 'general', message: 'one' });
      await store.save({ channel: 'random', message: 'two' });

      const drafts = await store.list();

      expect(drafts).toHaveLength(2);
    });
  });

  describe('get', () => {
    it('should return the draft with the given id', async () => {
      const saved = await store.save({ channel: 'general', message: 'hello' });

      const found = await store.get(saved.id);

      expect(found).toEqual(saved);
    });

    it('should return null when the draft does not exist', async () => {
      const found = await store.get('missing');

      expect(found).toBeNull();
    });
  });

  describe('delete', () => {
    it('should delete the draft with the given id', async () => {
      const saved = await store.save({ channel: 'general', message: 'hello' });

      await store.delete(saved.id);

      expect(await store.list()).toEqual([]);
    });

    it('should throw when the draft does not exist', async () => {
      await expect(store.delete('missing')).rejects.toThrow('Draft not found: missing');
    });

    it('should keep other drafts intact', async () => {
      const first = await store.save({ channel: 'general', message: 'one' });
      const second = await store.save({ channel: 'random', message: 'two' });

      await store.delete(first.id);

      const drafts = await store.list();
      expect(drafts).toHaveLength(1);
      expect(drafts[0].id).toBe(second.id);
    });
  });
});
