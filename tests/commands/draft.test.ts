import * as fs from 'fs/promises';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { setupDraftCommand } from '../../src/commands/draft';
import { DraftStorage } from '../../src/utils/draft-storage';
import { createTestProgram, restoreMocks, setupMockConsole } from '../test-utils';

vi.mock('../../src/utils/draft-storage');
vi.mock('fs/promises');

describe('draft command', () => {
  let program: ReturnType<typeof createTestProgram>;
  let mockStorage: DraftStorage;
  let mockConsole: ReturnType<typeof setupMockConsole>;

  const sampleDraft = {
    id: 'draft_1713340000000_abc123',
    channel: 'C0ARY9ESLCX',
    channelLabel: '#dev-morita-slack-test',
    thread: '1700000000.000100',
    message: 'お疲れさまです！',
    profile: 'default',
    note: '田中さんへの返信',
    createdAt: '2026-04-17T07:30:00.000Z',
    updatedAt: '2026-04-17T07:30:00.000Z',
  };

  beforeEach(() => {
    vi.clearAllMocks();

    mockStorage = new DraftStorage();
    vi.mocked(DraftStorage).mockImplementation(function () {
      return mockStorage;
    });

    mockConsole = setupMockConsole();
    program = createTestProgram();
    program.addCommand(setupDraftCommand());
  });

  afterEach(() => {
    restoreMocks();
  });

  describe('save subcommand', () => {
    it('saves a draft with channel and message', async () => {
      vi.mocked(mockStorage.save).mockResolvedValue(sampleDraft);

      await program.parseAsync([
        'node',
        'slack-cli',
        'draft',
        'save',
        '-c',
        'C0ARY9ESLCX',
        '-m',
        'お疲れさまです！',
      ]);

      expect(mockStorage.save).toHaveBeenCalledWith(
        expect.objectContaining({
          channel: 'C0ARY9ESLCX',
          message: 'お疲れさまです！',
          profile: 'default',
        })
      );
      expect(mockConsole.logSpy).toHaveBeenCalledWith(expect.stringContaining(sampleDraft.id));
    });

    it('saves a draft with thread, note, and channelLabel', async () => {
      vi.mocked(mockStorage.save).mockResolvedValue(sampleDraft);

      await program.parseAsync([
        'node',
        'slack-cli',
        'draft',
        'save',
        '-c',
        'C0ARY9ESLCX',
        '--channel-label',
        '#dev-morita-slack-test',
        '-t',
        '1700000000.000100',
        '-m',
        'body',
        '--note',
        '田中さんへの返信',
      ]);

      expect(mockStorage.save).toHaveBeenCalledWith(
        expect.objectContaining({
          channel: 'C0ARY9ESLCX',
          channelLabel: '#dev-morita-slack-test',
          thread: '1700000000.000100',
          message: 'body',
          note: '田中さんへの返信',
        })
      );
    });

    it('reads message body from file with -f', async () => {
      vi.mocked(fs.readFile).mockResolvedValue('file content body' as unknown as string);
      vi.mocked(mockStorage.save).mockResolvedValue(sampleDraft);

      await program.parseAsync([
        'node',
        'slack-cli',
        'draft',
        'save',
        '-c',
        'C0ARY9ESLCX',
        '-f',
        '/tmp/body.txt',
      ]);

      expect(fs.readFile).toHaveBeenCalledWith('/tmp/body.txt', 'utf-8');
      expect(mockStorage.save).toHaveBeenCalledWith(
        expect.objectContaining({ message: 'file content body' })
      );
    });

    it('passes blocks when --blocks-file is given', async () => {
      const blocks = [{ type: 'section', text: { type: 'mrkdwn', text: 'hi' } }];
      vi.mocked(fs.readFile).mockResolvedValue(JSON.stringify(blocks) as unknown as string);
      vi.mocked(mockStorage.save).mockResolvedValue(sampleDraft);

      await program.parseAsync([
        'node',
        'slack-cli',
        'draft',
        'save',
        '-c',
        'C0ARY9ESLCX',
        '-m',
        'fallback',
        '--blocks-file',
        '/tmp/blocks.json',
      ]);

      expect(mockStorage.save).toHaveBeenCalledWith(expect.objectContaining({ blocks }));
    });

    it('updates an existing draft when --id is given', async () => {
      vi.mocked(mockStorage.save).mockResolvedValue(sampleDraft);

      await program.parseAsync([
        'node',
        'slack-cli',
        'draft',
        'save',
        '--id',
        sampleDraft.id,
        '-c',
        'C0ARY9ESLCX',
        '-m',
        'updated body',
      ]);

      expect(mockStorage.save).toHaveBeenCalledWith(
        expect.objectContaining({
          id: sampleDraft.id,
          message: 'updated body',
        })
      );
    });

    it('prints JSON output when --format json is given', async () => {
      vi.mocked(mockStorage.save).mockResolvedValue(sampleDraft);

      await program.parseAsync([
        'node',
        'slack-cli',
        'draft',
        'save',
        '-c',
        'C0ARY9ESLCX',
        '-m',
        'x',
        '--format',
        'json',
      ]);

      const printed = mockConsole.logSpy.mock.calls.map((c) => String(c[0])).join('\n');
      const parsed = JSON.parse(printed);
      expect(parsed.id).toBe(sampleDraft.id);
    });

    it('errors when neither --message nor --file is given', async () => {
      await program.parseAsync(['node', 'slack-cli', 'draft', 'save', '-c', 'C0ARY9ESLCX']);

      expect(mockConsole.errorSpy).toHaveBeenCalled();
      expect(mockConsole.exitSpy).toHaveBeenCalledWith(1);
      expect(mockStorage.save).not.toHaveBeenCalled();
    });

    it('uses the specified profile', async () => {
      vi.mocked(mockStorage.save).mockResolvedValue(sampleDraft);

      await program.parseAsync([
        'node',
        'slack-cli',
        'draft',
        'save',
        '-c',
        'C0ARY9ESLCX',
        '-m',
        'body',
        '--profile',
        'work',
      ]);

      expect(mockStorage.save).toHaveBeenCalledWith(expect.objectContaining({ profile: 'work' }));
    });
  });

  describe('list subcommand', () => {
    it('lists drafts in table format by default', async () => {
      vi.mocked(mockStorage.list).mockResolvedValue([sampleDraft]);

      await program.parseAsync(['node', 'slack-cli', 'draft', 'list']);

      expect(mockStorage.list).toHaveBeenCalled();
      const output = mockConsole.logSpy.mock.calls.map((c) => String(c[0])).join('\n');
      expect(output).toContain(sampleDraft.id);
    });

    it('lists drafts in json format', async () => {
      vi.mocked(mockStorage.list).mockResolvedValue([sampleDraft]);

      await program.parseAsync(['node', 'slack-cli', 'draft', 'list', '--format', 'json']);

      const output = mockConsole.logSpy.mock.calls.map((c) => String(c[0])).join('\n');
      const parsed = JSON.parse(output);
      expect(parsed).toHaveLength(1);
      expect(parsed[0].id).toBe(sampleDraft.id);
    });

    it('shows message when no drafts are found', async () => {
      vi.mocked(mockStorage.list).mockResolvedValue([]);

      await program.parseAsync(['node', 'slack-cli', 'draft', 'list']);

      expect(mockConsole.logSpy).toHaveBeenCalledWith('No drafts found');
    });
  });

  describe('show subcommand', () => {
    it('prints a draft by id', async () => {
      vi.mocked(mockStorage.get).mockResolvedValue(sampleDraft);

      await program.parseAsync(['node', 'slack-cli', 'draft', 'show', sampleDraft.id]);

      expect(mockStorage.get).toHaveBeenCalledWith(sampleDraft.id);
      const output = mockConsole.logSpy.mock.calls.map((c) => String(c[0])).join('\n');
      expect(output).toContain(sampleDraft.message);
      expect(output).toContain(sampleDraft.channel);
    });

    it('prints draft as JSON with --format json', async () => {
      vi.mocked(mockStorage.get).mockResolvedValue(sampleDraft);

      await program.parseAsync([
        'node',
        'slack-cli',
        'draft',
        'show',
        sampleDraft.id,
        '--format',
        'json',
      ]);

      const output = mockConsole.logSpy.mock.calls.map((c) => String(c[0])).join('\n');
      const parsed = JSON.parse(output);
      expect(parsed.id).toBe(sampleDraft.id);
      expect(parsed.message).toBe(sampleDraft.message);
    });

    it('errors when draft does not exist', async () => {
      vi.mocked(mockStorage.get).mockResolvedValue(null);

      await program.parseAsync(['node', 'slack-cli', 'draft', 'show', 'draft_missing']);

      expect(mockConsole.errorSpy).toHaveBeenCalled();
      expect(mockConsole.exitSpy).toHaveBeenCalledWith(1);
    });
  });

  describe('delete subcommand', () => {
    it('deletes a draft by id', async () => {
      vi.mocked(mockStorage.delete).mockResolvedValue();

      await program.parseAsync(['node', 'slack-cli', 'draft', 'delete', sampleDraft.id]);

      expect(mockStorage.delete).toHaveBeenCalledWith(sampleDraft.id);
      expect(mockConsole.logSpy).toHaveBeenCalledWith(expect.stringContaining('Deleted'));
    });

    it('errors when draft does not exist', async () => {
      vi.mocked(mockStorage.delete).mockRejectedValue(new Error('Draft not found: x'));

      await program.parseAsync(['node', 'slack-cli', 'draft', 'delete', 'draft_missing']);

      expect(mockConsole.errorSpy).toHaveBeenCalled();
      expect(mockConsole.exitSpy).toHaveBeenCalledWith(1);
    });
  });
});
