import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { Channel } from '../../../src/types/slack';
import { createChannelFormatter } from '../../../src/utils/formatters/channel-formatters';

describe('channel formatters', () => {
  let logSpy: ReturnType<typeof vi.spyOn>;

  beforeEach(() => {
    logSpy = vi.spyOn(console, 'log').mockImplementation(() => undefined);
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  const channelsWithNewline: Channel[] = [
    {
      id: 'C123',
      name: 'general',
      display_name: 'general\nfake-row',
      is_private: false,
      created: 1709290800,
      unread_count: 2,
      last_read: '1709290800.000100',
    },
  ];

  describe('single-line output hardening', () => {
    it('should collapse newlines in table format channel names', () => {
      const formatter = createChannelFormatter('table', false);
      formatter.format({ channels: channelsWithNewline });

      const dataRow = logSpy.mock.calls
        .map((call) => call[0] as string)
        .find((line) => line.includes('general'));
      expect(dataRow).toBeDefined();
      expect(dataRow).not.toContain('\n');
      expect(dataRow).toContain('general fake-row');
    });

    it('should collapse newlines in simple format channel names', () => {
      const formatter = createChannelFormatter('simple', false);
      formatter.format({ channels: channelsWithNewline });

      const output = logSpy.mock.calls[0][0] as string;
      expect(output).not.toContain('\n');
      expect(output).toContain('general fake-row');
    });

    it('should collapse newlines in count format channel names', () => {
      const formatter = createChannelFormatter('count', true);
      formatter.format({ channels: channelsWithNewline });

      const countRow = logSpy.mock.calls
        .map((call) => call[0] as string)
        .find((line) => line.includes('general'));
      expect(countRow).toBeDefined();
      expect(countRow).not.toContain('\n');
    });
  });
});
