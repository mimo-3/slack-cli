import { beforeEach, describe, expect, it, vi } from 'vitest';
import { channelResolver } from '../../../src/utils/channel-resolver';
import { FileOperations } from '../../../src/utils/slack-operations/file-operations';

vi.mock('@slack/web-api', () => ({
  WebClient: vi.fn().mockImplementation(function () {
    return {
      files: {
        uploadV2: vi.fn(),
      },
    };
  }),
  LogLevel: {
    ERROR: 'error',
  },
}));

vi.mock('../../../src/utils/channel-resolver');

describe('FileOperations', () => {
  type MockClient = {
    files: {
      uploadV2: ReturnType<typeof vi.fn>;
    };
  };

  let fileOps: FileOperations;
  let mockClient: MockClient;

  beforeEach(() => {
    vi.clearAllMocks();
    fileOps = new FileOperations('test-token');
    mockClient = (fileOps as unknown as { client: MockClient }).client;
  });

  describe('uploadFile', () => {
    it('should upload a file by path', async () => {
      mockClient.files.uploadV2.mockResolvedValue({ ok: true });
      vi.mocked(channelResolver.resolveChannelId).mockResolvedValue('C123456789');

      await fileOps.uploadFile({
        channel: 'general',
        filePath: '/path/to/file.txt',
      });

      expect(channelResolver.resolveChannelId).toHaveBeenCalledWith(
        'general',
        expect.any(Function)
      );
      expect(mockClient.files.uploadV2).toHaveBeenCalledWith({
        channel_id: 'C123456789',
        file: '/path/to/file.txt',
        filename: 'file.txt',
      });
    });

    it('should upload content as snippet', async () => {
      mockClient.files.uploadV2.mockResolvedValue({ ok: true });
      vi.mocked(channelResolver.resolveChannelId).mockResolvedValue('C123456789');

      await fileOps.uploadFile({
        channel: 'general',
        content: 'console.log("hello")',
        filename: 'snippet.js',
      });

      expect(mockClient.files.uploadV2).toHaveBeenCalledWith({
        channel_id: 'C123456789',
        content: 'console.log("hello")',
        filename: 'snippet.js',
      });
    });

    it('should include optional parameters', async () => {
      mockClient.files.uploadV2.mockResolvedValue({ ok: true });
      vi.mocked(channelResolver.resolveChannelId).mockResolvedValue('C123456789');

      await fileOps.uploadFile({
        channel: 'general',
        filePath: '/path/to/report.csv',
        title: 'Daily Report',
        initialComment: 'Here is the report',
        snippetType: 'csv',
        threadTs: '1234567890.123456',
      });

      expect(mockClient.files.uploadV2).toHaveBeenCalledWith({
        channel_id: 'C123456789',
        file: '/path/to/report.csv',
        filename: 'report.csv',
        title: 'Daily Report',
        initial_comment: 'Here is the report',
        snippet_type: 'csv',
        thread_ts: '1234567890.123456',
      });
    });

    it('should use provided filename over path basename', async () => {
      mockClient.files.uploadV2.mockResolvedValue({ ok: true });
      vi.mocked(channelResolver.resolveChannelId).mockResolvedValue('C123456789');

      await fileOps.uploadFile({
        channel: 'general',
        filePath: '/path/to/file.txt',
        filename: 'custom-name.txt',
      });

      expect(mockClient.files.uploadV2).toHaveBeenCalledWith({
        channel_id: 'C123456789',
        file: '/path/to/file.txt',
        filename: 'custom-name.txt',
      });
    });

    it('should throw on API error', async () => {
      mockClient.files.uploadV2.mockRejectedValue(new Error('not_allowed_token_type'));
      vi.mocked(channelResolver.resolveChannelId).mockResolvedValue('C123456789');

      await expect(
        fileOps.uploadFile({
          channel: 'general',
          filePath: '/path/to/file.txt',
        })
      ).rejects.toThrow('not_allowed_token_type');
    });

    it('should collect files from nested response.files[].files[]', async () => {
      mockClient.files.uploadV2.mockResolvedValue({
        ok: true,
        files: [
          { ok: true, files: [{ id: 'F1', permalink: 'https://example.com/1' }] },
          { ok: true, files: [{ id: 'F2', permalink: 'https://example.com/2' }] },
        ],
      });
      vi.mocked(channelResolver.resolveChannelId).mockResolvedValue('C123456789');

      const result = await fileOps.uploadFile({
        channel: 'general',
        filePath: '/path/to/file.txt',
      });

      expect(result.files).toEqual([
        { id: 'F1', permalink: 'https://example.com/1' },
        { id: 'F2', permalink: 'https://example.com/2' },
      ]);
    });

    it('should return empty files array when response.files is missing', async () => {
      mockClient.files.uploadV2.mockResolvedValue({ ok: true });
      vi.mocked(channelResolver.resolveChannelId).mockResolvedValue('C123456789');

      const result = await fileOps.uploadFile({
        channel: 'general',
        filePath: '/path/to/file.txt',
      });

      expect(result.files).toEqual([]);
    });

    it('should throw when response.ok is false', async () => {
      mockClient.files.uploadV2.mockResolvedValue({ ok: false, error: 'not_in_channel' });
      vi.mocked(channelResolver.resolveChannelId).mockResolvedValue('C123456789');

      await expect(
        fileOps.uploadFile({
          channel: 'general',
          filePath: '/path/to/file.txt',
        })
      ).rejects.toThrow('not_in_channel');
    });

    it('should throw when an entry has ok: false', async () => {
      mockClient.files.uploadV2.mockResolvedValue({
        ok: true,
        files: [{ ok: false, error: 'file_too_large' }],
      });
      vi.mocked(channelResolver.resolveChannelId).mockResolvedValue('C123456789');

      await expect(
        fileOps.uploadFile({
          channel: 'general',
          filePath: '/path/to/file.txt',
        })
      ).rejects.toThrow('file_too_large');
    });
  });
});
