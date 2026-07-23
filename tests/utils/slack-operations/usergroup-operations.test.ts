import { beforeEach, describe, expect, it, vi } from 'vitest';
import { UsergroupOperations } from '../../../src/utils/slack-operations/usergroup-operations';

vi.mock('@slack/web-api', () => ({
  WebClient: vi.fn().mockImplementation(function () {
    return {
      usergroups: {
        list: vi.fn(),
        users: {
          list: vi.fn(),
        },
      },
    };
  }),
  LogLevel: {
    ERROR: 'error',
  },
}));

describe('UsergroupOperations', () => {
  type MockClient = {
    usergroups: {
      list: ReturnType<typeof vi.fn>;
      users: {
        list: ReturnType<typeof vi.fn>;
      };
    };
  };

  let usergroupOps: UsergroupOperations;
  let mockClient: MockClient;

  beforeEach(() => {
    vi.clearAllMocks();
    usergroupOps = new UsergroupOperations('test-token');
    mockClient = (usergroupOps as unknown as { client: MockClient }).client;
  });

  describe('listUsergroups', () => {
    it('should list usergroups with member counts', async () => {
      mockClient.usergroups.list.mockResolvedValue({
        ok: true,
        usergroups: [
          {
            id: 'S123',
            name: 'Engineering',
            handle: 'engineers',
            description: 'Engineering team',
            user_count: 10,
            date_delete: 0,
          },
          {
            id: 'S456',
            name: 'Design',
            handle: 'designers',
            description: 'Design team',
            user_count: 5,
            date_delete: 0,
          },
        ],
      });

      const result = await usergroupOps.listUsergroups();

      expect(mockClient.usergroups.list).toHaveBeenCalledWith({
        include_count: true,
      });
      expect(result).toHaveLength(2);
      expect(result[0].id).toBe('S123');
      expect(result[0].handle).toBe('engineers');
      expect(result[1].id).toBe('S456');
    });

    it('should include disabled usergroups when requested', async () => {
      mockClient.usergroups.list.mockResolvedValue({
        ok: true,
        usergroups: [],
      });

      await usergroupOps.listUsergroups(true);

      expect(mockClient.usergroups.list).toHaveBeenCalledWith({
        include_count: true,
        include_disabled: true,
      });
    });

    it('should return empty array when no usergroups exist', async () => {
      mockClient.usergroups.list.mockResolvedValue({
        ok: true,
        usergroups: [],
      });

      const result = await usergroupOps.listUsergroups();

      expect(result).toEqual([]);
    });

    it('should return empty array when usergroups field is missing', async () => {
      mockClient.usergroups.list.mockResolvedValue({
        ok: true,
      });

      const result = await usergroupOps.listUsergroups();

      expect(result).toEqual([]);
    });

    it('should throw when API call fails', async () => {
      mockClient.usergroups.list.mockRejectedValue(new Error('missing_scope'));

      await expect(usergroupOps.listUsergroups()).rejects.toThrow('missing_scope');
    });
  });

  describe('listUsergroupUsers', () => {
    it('should list user IDs in a usergroup', async () => {
      mockClient.usergroups.users.list.mockResolvedValue({
        ok: true,
        users: ['U123', 'U456'],
      });

      const result = await usergroupOps.listUsergroupUsers('S123');

      expect(mockClient.usergroups.users.list).toHaveBeenCalledWith({
        usergroup: 'S123',
      });
      expect(result).toEqual(['U123', 'U456']);
    });

    it('should return empty array when usergroup has no users', async () => {
      mockClient.usergroups.users.list.mockResolvedValue({
        ok: true,
        users: [],
      });

      const result = await usergroupOps.listUsergroupUsers('S123');

      expect(result).toEqual([]);
    });

    it('should throw when usergroup not found', async () => {
      mockClient.usergroups.users.list.mockRejectedValue(new Error('no_such_subteam'));

      await expect(usergroupOps.listUsergroupUsers('SINVALID')).rejects.toThrow('no_such_subteam');
    });
  });

  describe('resolveUsergroupIdByHandle', () => {
    it('should resolve usergroup ID by handle', async () => {
      mockClient.usergroups.list.mockResolvedValue({
        ok: true,
        usergroups: [
          { id: 'S123', name: 'Engineering', handle: 'engineers' },
          { id: 'S456', name: 'Design', handle: 'designers' },
        ],
      });

      const result = await usergroupOps.resolveUsergroupIdByHandle('designers');

      expect(result).toBe('S456');
    });

    it('should resolve handle with @ prefix', async () => {
      mockClient.usergroups.list.mockResolvedValue({
        ok: true,
        usergroups: [{ id: 'S123', name: 'Engineering', handle: 'engineers' }],
      });

      const result = await usergroupOps.resolveUsergroupIdByHandle('@engineers');

      expect(result).toBe('S123');
    });

    it('should resolve handle case-insensitively', async () => {
      mockClient.usergroups.list.mockResolvedValue({
        ok: true,
        usergroups: [{ id: 'S123', name: 'Engineering', handle: 'Engineers' }],
      });

      const result = await usergroupOps.resolveUsergroupIdByHandle('engineers');

      expect(result).toBe('S123');
    });

    it('should throw when handle not found', async () => {
      mockClient.usergroups.list.mockResolvedValue({
        ok: true,
        usergroups: [{ id: 'S123', name: 'Engineering', handle: 'engineers' }],
      });

      await expect(usergroupOps.resolveUsergroupIdByHandle('unknown')).rejects.toThrow(
        "Usergroup '@unknown' not found"
      );
    });
  });
});
