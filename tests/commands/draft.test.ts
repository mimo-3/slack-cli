import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { setupDraftCommand } from '../../src/commands/draft';
import { DraftStore } from '../../src/utils/draft-store';
import { ProfileConfigManager } from '../../src/utils/profile-config';
import { SlackApiClient } from '../../src/utils/slack-api-client';
import { createTestProgram, restoreMocks, setupMockConsole } from '../test-utils';

vi.mock('../../src/utils/slack-api-client');
vi.mock('../../src/utils/profile-config');
vi.mock('../../src/utils/draft-store');

describe('draft command', () => {
  let program: ReturnType<typeof createTestProgram>;
  let mockSlackClient: SlackApiClient;
  let mockConfigManager: ProfileConfigManager;
  let mockDraftStore: DraftStore;
  let mockConsole: ReturnType<typeof setupMockConsole>;
  let tableSpy: ReturnType<typeof vi.spyOn>;

  const sampleDraft = {
    id: 'draft-1',
    channel: 'general',
    message: 'hello world',
    createdAt: '2026-07-16T00:00:00.000Z',
  };

  beforeEach(() => {
    vi.clearAllMocks();

    mockConfigManager = new ProfileConfigManager();
    vi.mocked(ProfileConfigManager).mockImplementation(function () {
      return mockConfigManager;
    });
    vi.mocked(mockConfigManager.getConfig).mockResolvedValue({
      token: 'test-token',
      updatedAt: new Date().toISOString(),
    });

    mockSlackClient = new SlackApiClient('test-token');
    vi.mocked(SlackApiClient).mockImplementation(function () {
      return mockSlackClient;
    });

    mockDraftStore = new DraftStore();
    vi.mocked(DraftStore).mockImplementation(function () {
      return mockDraftStore;
    });

    mockConsole = setupMockConsole();
    tableSpy = vi.spyOn(console, 'table').mockImplementation(() => undefined);

    program = createTestProgram();
    program.addCommand(setupDraftCommand());
  });

  afterEach(() => {
    restoreMocks();
  });

  describe('save subcommand', () => {
    it('should save a draft for a channel', async () => {
      vi.mocked(mockDraftStore.save).mockResolvedValue(sampleDraft);

      await program.parseAsync([
        'node',
        'slack-cli',
        'draft',
        'save',
        '-c',
        'general',
        '-m',
        'hello world',
      ]);

      expect(mockDraftStore.save).toHaveBeenCalledWith({
        channel: 'general',
        message: 'hello world',
        thread: undefined,
        user: undefined,
      });
      expect(mockConsole.logSpy).toHaveBeenCalledWith(expect.stringContaining('draft-1'));
    });

    it('should save a draft for a user DM', async () => {
      vi.mocked(mockDraftStore.save).mockResolvedValue({
        ...sampleDraft,
        channel: undefined,
        user: 'alice',
      });

      await program.parseAsync([
        'node',
        'slack-cli',
        'draft',
        'save',
        '--user',
        'alice',
        '-m',
        'hello world',
      ]);

      expect(mockDraftStore.save).toHaveBeenCalledWith({
        channel: undefined,
        message: 'hello world',
        thread: undefined,
        user: 'alice',
      });
    });

    it('should fail when neither channel nor user is specified', async () => {
      await program.parseAsync(['node', 'slack-cli', 'draft', 'save', '-m', 'hello']);

      expect(mockDraftStore.save).not.toHaveBeenCalled();
      expect(mockConsole.errorSpy).toHaveBeenCalled();
    });

    it('should fail when message is missing', async () => {
      await program.parseAsync(['node', 'slack-cli', 'draft', 'save', '-c', 'general']);

      expect(mockDraftStore.save).not.toHaveBeenCalled();
      expect(mockConsole.errorSpy).toHaveBeenCalled();
    });
  });

  describe('list subcommand', () => {
    it('should list drafts in table format by default', async () => {
      vi.mocked(mockDraftStore.list).mockResolvedValue([sampleDraft]);

      await program.parseAsync(['node', 'slack-cli', 'draft', 'list']);

      expect(tableSpy).toHaveBeenCalled();
    });

    it('should print a message when there are no drafts', async () => {
      vi.mocked(mockDraftStore.list).mockResolvedValue([]);

      await program.parseAsync(['node', 'slack-cli', 'draft', 'list']);

      expect(mockConsole.logSpy).toHaveBeenCalledWith('No drafts found');
      expect(tableSpy).not.toHaveBeenCalled();
    });
  });

  describe('show subcommand', () => {
    it('should show the full draft content', async () => {
      vi.mocked(mockDraftStore.get).mockResolvedValue(sampleDraft);

      await program.parseAsync(['node', 'slack-cli', 'draft', 'show', '--id', 'draft-1']);

      expect(mockDraftStore.get).toHaveBeenCalledWith('draft-1');
      expect(mockConsole.logSpy).toHaveBeenCalledWith(expect.stringContaining('hello world'));
    });

    it('should error when the draft does not exist', async () => {
      vi.mocked(mockDraftStore.get).mockResolvedValue(null);

      await program.parseAsync(['node', 'slack-cli', 'draft', 'show', '--id', 'missing']);

      expect(mockConsole.errorSpy).toHaveBeenCalled();
    });
  });

  describe('send subcommand', () => {
    it('should send a channel draft and delete it on success', async () => {
      vi.mocked(mockDraftStore.get).mockResolvedValue(sampleDraft);
      vi.mocked(mockSlackClient.sendMessage).mockResolvedValue({ ok: true });

      await program.parseAsync(['node', 'slack-cli', 'draft', 'send', '--id', 'draft-1']);

      expect(mockSlackClient.sendMessage).toHaveBeenCalledWith('general', 'hello world', undefined);
      expect(mockDraftStore.delete).toHaveBeenCalledWith('draft-1');
    });

    it('should send a DM draft by resolving the user', async () => {
      vi.mocked(mockDraftStore.get).mockResolvedValue({
        ...sampleDraft,
        channel: undefined,
        user: 'alice',
      });
      vi.mocked(mockSlackClient.resolveUserIdByName).mockResolvedValue('U123');
      vi.mocked(mockSlackClient.openDmChannel).mockResolvedValue('D123');
      vi.mocked(mockSlackClient.sendMessage).mockResolvedValue({ ok: true });

      await program.parseAsync(['node', 'slack-cli', 'draft', 'send', '--id', 'draft-1']);

      expect(mockSlackClient.resolveUserIdByName).toHaveBeenCalledWith('alice');
      expect(mockSlackClient.sendMessage).toHaveBeenCalledWith('D123', 'hello world', undefined);
      expect(mockDraftStore.delete).toHaveBeenCalledWith('draft-1');
    });

    it('should keep the draft with --keep', async () => {
      vi.mocked(mockDraftStore.get).mockResolvedValue(sampleDraft);
      vi.mocked(mockSlackClient.sendMessage).mockResolvedValue({ ok: true });

      await program.parseAsync(['node', 'slack-cli', 'draft', 'send', '--id', 'draft-1', '--keep']);

      expect(mockSlackClient.sendMessage).toHaveBeenCalled();
      expect(mockDraftStore.delete).not.toHaveBeenCalled();
    });

    it('should not delete the draft when sending fails', async () => {
      vi.mocked(mockDraftStore.get).mockResolvedValue(sampleDraft);
      vi.mocked(mockSlackClient.sendMessage).mockRejectedValue(new Error('network error'));

      await program.parseAsync(['node', 'slack-cli', 'draft', 'send', '--id', 'draft-1']);

      expect(mockDraftStore.delete).not.toHaveBeenCalled();
      expect(mockConsole.errorSpy).toHaveBeenCalled();
    });

    it('should error when the draft does not exist', async () => {
      vi.mocked(mockDraftStore.get).mockResolvedValue(null);

      await program.parseAsync(['node', 'slack-cli', 'draft', 'send', '--id', 'missing']);

      expect(mockSlackClient.sendMessage).not.toHaveBeenCalled();
      expect(mockConsole.errorSpy).toHaveBeenCalled();
    });
  });

  describe('delete subcommand', () => {
    it('should delete a draft', async () => {
      vi.mocked(mockDraftStore.delete).mockResolvedValue(undefined);

      await program.parseAsync(['node', 'slack-cli', 'draft', 'delete', '--id', 'draft-1']);

      expect(mockDraftStore.delete).toHaveBeenCalledWith('draft-1');
      expect(mockConsole.logSpy).toHaveBeenCalledWith(expect.stringContaining('draft-1'));
    });
  });
});
