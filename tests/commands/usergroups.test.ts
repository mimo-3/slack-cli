import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { setupUsergroupsCommand } from '../../src/commands/usergroups';
import { ProfileConfigManager } from '../../src/utils/profile-config';
import { SlackApiClient } from '../../src/utils/slack-api-client';
import { createTestProgram, restoreMocks, setupMockConsole } from '../test-utils';

vi.mock('../../src/utils/slack-api-client');
vi.mock('../../src/utils/profile-config');

describe('usergroups command', () => {
  let program: ReturnType<typeof createTestProgram>;
  let mockSlackClient: SlackApiClient;
  let mockConfigManager: ProfileConfigManager;
  let mockConsole: ReturnType<typeof setupMockConsole>;

  beforeEach(() => {
    vi.clearAllMocks();

    mockConfigManager = new ProfileConfigManager();
    vi.mocked(ProfileConfigManager).mockImplementation(function () {
      return mockConfigManager;
    });

    mockSlackClient = new SlackApiClient('test-token');
    vi.mocked(SlackApiClient).mockImplementation(function () {
      return mockSlackClient;
    });

    mockConsole = setupMockConsole();
    program = createTestProgram();
    program.addCommand(setupUsergroupsCommand());
  });

  afterEach(() => {
    restoreMocks();
  });

  describe('list subcommand', () => {
    it('should list usergroups in table format', async () => {
      vi.mocked(mockConfigManager.getConfig).mockResolvedValue({
        token: 'test-token',
        updatedAt: new Date().toISOString(),
      });
      vi.mocked(mockSlackClient.listUsergroups).mockResolvedValue([
        {
          id: 'S123',
          name: 'Engineering',
          handle: 'engineers',
          description: 'Engineering team',
          user_count: 10,
        },
      ]);

      await program.parseAsync(['node', 'slack-cli', 'usergroups', 'list']);

      expect(mockSlackClient.listUsergroups).toHaveBeenCalledWith(false);
      expect(mockConsole.logSpy).toHaveBeenCalled();
    });

    it('should include disabled usergroups with --include-disabled', async () => {
      vi.mocked(mockConfigManager.getConfig).mockResolvedValue({
        token: 'test-token',
        updatedAt: new Date().toISOString(),
      });
      vi.mocked(mockSlackClient.listUsergroups).mockResolvedValue([]);

      await program.parseAsync(['node', 'slack-cli', 'usergroups', 'list', '--include-disabled']);

      expect(mockSlackClient.listUsergroups).toHaveBeenCalledWith(true);
    });

    it('should list usergroups in json format', async () => {
      vi.mocked(mockConfigManager.getConfig).mockResolvedValue({
        token: 'test-token',
        updatedAt: new Date().toISOString(),
      });
      const usergroups = [
        {
          id: 'S123',
          name: 'Engineering',
          handle: 'engineers',
          description: 'Engineering team',
          user_count: 10,
        },
      ];
      vi.mocked(mockSlackClient.listUsergroups).mockResolvedValue(usergroups);

      await program.parseAsync(['node', 'slack-cli', 'usergroups', 'list', '--format', 'json']);

      expect(mockConsole.logSpy).toHaveBeenCalledWith(JSON.stringify(usergroups, null, 2));
    });

    it('should list usergroups in simple format', async () => {
      vi.mocked(mockConfigManager.getConfig).mockResolvedValue({
        token: 'test-token',
        updatedAt: new Date().toISOString(),
      });
      vi.mocked(mockSlackClient.listUsergroups).mockResolvedValue([
        {
          id: 'S123',
          name: 'Engineering',
          handle: 'engineers',
          description: 'Engineering team',
          user_count: 10,
        },
      ]);

      await program.parseAsync(['node', 'slack-cli', 'usergroups', 'list', '--format', 'simple']);

      expect(mockConsole.logSpy).toHaveBeenCalledWith(expect.stringContaining('S123'));
      expect(mockConsole.logSpy).toHaveBeenCalledWith(expect.stringContaining('engineers'));
    });

    it('should collapse newlines in simple format fields', async () => {
      vi.mocked(mockConfigManager.getConfig).mockResolvedValue({
        token: 'test-token',
        updatedAt: new Date().toISOString(),
      });
      vi.mocked(mockSlackClient.listUsergroups).mockResolvedValue([
        {
          id: 'S123',
          name: 'Engineering\nfake-row',
          handle: 'eng\tineers',
          description: 'team',
          user_count: 10,
        },
      ]);

      await program.parseAsync(['node', 'slack-cli', 'usergroups', 'list', '--format', 'simple']);

      const output = mockConsole.logSpy.mock.calls[0][0] as string;
      expect(output).not.toContain('\n');
      expect(output).toContain('Engineering fake-row');
      expect(output).toContain('eng ineers');
    });

    it('should show message when no usergroups found', async () => {
      vi.mocked(mockConfigManager.getConfig).mockResolvedValue({
        token: 'test-token',
        updatedAt: new Date().toISOString(),
      });
      vi.mocked(mockSlackClient.listUsergroups).mockResolvedValue([]);

      await program.parseAsync(['node', 'slack-cli', 'usergroups', 'list']);

      expect(mockConsole.logSpy).toHaveBeenCalledWith('No usergroups found');
    });

    it('should use specified profile', async () => {
      vi.mocked(mockConfigManager.getConfig).mockResolvedValue({
        token: 'work-token',
        updatedAt: new Date().toISOString(),
      });
      vi.mocked(mockSlackClient.listUsergroups).mockResolvedValue([]);

      await program.parseAsync(['node', 'slack-cli', 'usergroups', 'list', '--profile', 'work']);

      expect(mockConfigManager.getConfig).toHaveBeenCalledWith('work');
      expect(SlackApiClient).toHaveBeenCalledWith('work-token');
    });

    it('should handle missing scope error', async () => {
      vi.mocked(mockConfigManager.getConfig).mockResolvedValue({
        token: 'test-token',
        updatedAt: new Date().toISOString(),
      });
      vi.mocked(mockSlackClient.listUsergroups).mockRejectedValue(new Error('missing_scope'));

      await program.parseAsync(['node', 'slack-cli', 'usergroups', 'list']);

      expect(mockConsole.errorSpy).toHaveBeenCalledWith(
        expect.stringContaining('Error:'),
        expect.any(String)
      );
      expect(mockConsole.exitSpy).toHaveBeenCalledWith(1);
    });
  });

  describe('members subcommand', () => {
    it('should list usergroup members by id', async () => {
      vi.mocked(mockConfigManager.getConfig).mockResolvedValue({
        token: 'test-token',
        updatedAt: new Date().toISOString(),
      });
      vi.mocked(mockSlackClient.listUsergroupMembers).mockResolvedValue(['U123', 'U456']);
      vi.mocked(mockSlackClient.getUserInfo).mockImplementation(async (userId: string) => ({
        id: userId,
        name: userId === 'U123' ? 'alice' : 'bob',
        real_name: userId === 'U123' ? 'Alice Smith' : 'Bob Jones',
      }));

      await program.parseAsync(['node', 'slack-cli', 'usergroups', 'members', '--id', 'S123']);

      expect(mockSlackClient.listUsergroupMembers).toHaveBeenCalledWith('S123');
      expect(mockSlackClient.getUserInfo).toHaveBeenCalledWith('U123');
      expect(mockSlackClient.getUserInfo).toHaveBeenCalledWith('U456');
      expect(mockConsole.logSpy).toHaveBeenCalled();
    });

    it('should list usergroup members by handle', async () => {
      vi.mocked(mockConfigManager.getConfig).mockResolvedValue({
        token: 'test-token',
        updatedAt: new Date().toISOString(),
      });
      vi.mocked(mockSlackClient.resolveUsergroupIdByHandle).mockResolvedValue('S123');
      vi.mocked(mockSlackClient.listUsergroupMembers).mockResolvedValue(['U123']);
      vi.mocked(mockSlackClient.getUserInfo).mockResolvedValue({
        id: 'U123',
        name: 'alice',
        real_name: 'Alice Smith',
      });

      await program.parseAsync([
        'node',
        'slack-cli',
        'usergroups',
        'members',
        '--handle',
        '@engineers',
      ]);

      expect(mockSlackClient.resolveUsergroupIdByHandle).toHaveBeenCalledWith('@engineers');
      expect(mockSlackClient.listUsergroupMembers).toHaveBeenCalledWith('S123');
    });

    it('should list members in simple format', async () => {
      vi.mocked(mockConfigManager.getConfig).mockResolvedValue({
        token: 'test-token',
        updatedAt: new Date().toISOString(),
      });
      vi.mocked(mockSlackClient.listUsergroupMembers).mockResolvedValue(['U123']);
      vi.mocked(mockSlackClient.getUserInfo).mockResolvedValue({
        id: 'U123',
        name: 'alice',
        real_name: 'Alice Smith',
      });

      await program.parseAsync([
        'node',
        'slack-cli',
        'usergroups',
        'members',
        '--id',
        'S123',
        '--format',
        'simple',
      ]);

      expect(mockConsole.logSpy).toHaveBeenCalledWith(expect.stringContaining('U123'));
      expect(mockConsole.logSpy).toHaveBeenCalledWith(expect.stringContaining('alice'));
    });

    it('should fall back to user ID when user info lookup fails', async () => {
      vi.mocked(mockConfigManager.getConfig).mockResolvedValue({
        token: 'test-token',
        updatedAt: new Date().toISOString(),
      });
      vi.mocked(mockSlackClient.listUsergroupMembers).mockResolvedValue(['U123']);
      vi.mocked(mockSlackClient.getUserInfo).mockRejectedValue(new Error('user_not_found'));

      await program.parseAsync([
        'node',
        'slack-cli',
        'usergroups',
        'members',
        '--id',
        'S123',
        '--format',
        'simple',
      ]);

      expect(mockConsole.logSpy).toHaveBeenCalledWith(expect.stringContaining('U123'));
      expect(mockConsole.exitSpy).not.toHaveBeenCalled();
    });

    it('should show message when usergroup has no members', async () => {
      vi.mocked(mockConfigManager.getConfig).mockResolvedValue({
        token: 'test-token',
        updatedAt: new Date().toISOString(),
      });
      vi.mocked(mockSlackClient.listUsergroupMembers).mockResolvedValue([]);

      await program.parseAsync(['node', 'slack-cli', 'usergroups', 'members', '--id', 'S123']);

      expect(mockConsole.logSpy).toHaveBeenCalledWith('No members found');
    });

    it('should error when neither --id nor --handle is specified', async () => {
      vi.mocked(mockConfigManager.getConfig).mockResolvedValue({
        token: 'test-token',
        updatedAt: new Date().toISOString(),
      });

      await program.parseAsync(['node', 'slack-cli', 'usergroups', 'members']);

      expect(mockConsole.errorSpy).toHaveBeenCalledWith(
        expect.stringContaining('Error:'),
        expect.any(String)
      );
      expect(mockConsole.exitSpy).toHaveBeenCalledWith(1);
    });

    it('should error when both --id and --handle are specified', async () => {
      vi.mocked(mockConfigManager.getConfig).mockResolvedValue({
        token: 'test-token',
        updatedAt: new Date().toISOString(),
      });

      await program.parseAsync([
        'node',
        'slack-cli',
        'usergroups',
        'members',
        '--id',
        'S123',
        '--handle',
        'engineers',
      ]);

      expect(mockConsole.errorSpy).toHaveBeenCalledWith(
        expect.stringContaining('Error:'),
        expect.any(String)
      );
      expect(mockConsole.exitSpy).toHaveBeenCalledWith(1);
    });

    it('should handle usergroup not found error', async () => {
      vi.mocked(mockConfigManager.getConfig).mockResolvedValue({
        token: 'test-token',
        updatedAt: new Date().toISOString(),
      });
      vi.mocked(mockSlackClient.listUsergroupMembers).mockRejectedValue(
        new Error('no_such_subteam')
      );

      await program.parseAsync(['node', 'slack-cli', 'usergroups', 'members', '--id', 'SINVALID']);

      expect(mockConsole.errorSpy).toHaveBeenCalledWith(
        expect.stringContaining('Error:'),
        expect.any(String)
      );
      expect(mockConsole.exitSpy).toHaveBeenCalledWith(1);
    });
  });

  describe('error handling', () => {
    it('should handle missing configuration', async () => {
      vi.mocked(mockConfigManager.getConfig).mockResolvedValue(null);

      await program.parseAsync(['node', 'slack-cli', 'usergroups', 'list']);

      expect(mockConsole.errorSpy).toHaveBeenCalledWith(
        expect.stringContaining('Error:'),
        expect.any(String)
      );
      expect(mockConsole.exitSpy).toHaveBeenCalledWith(1);
    });
  });
});
