import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createChannelsListFormatter } from '../../../src/utils/formatters/channels-list-formatters';

describe('channels list formatters', () => {
  let logSpy: ReturnType<typeof vi.spyOn>;

  beforeEach(() => {
    logSpy = vi.spyOn(console, 'log').mockImplementation(() => undefined);
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  const channelsWithNewline = [
    {
      id: 'C123',
      name: 'general\nfake-row',
      type: 'public',
      members: 10,
      created: '2026-01-01',
      purpose: 'first line\nsecond line',
      is_archived: false,
    },
  ];

  describe('single-line output hardening', () => {
    it('should collapse newlines in table format fields', () => {
      const formatter = createChannelsListFormatter('table');
      formatter.format({ channels: channelsWithNewline });

      const dataRow = logSpy.mock.calls
        .map((call) => call[0] as string)
        .find((line) => line.includes('general'));
      expect(dataRow).toBeDefined();
      expect(dataRow).not.toContain('\n');
      expect(dataRow).toContain('general fake-row');
    });

    it('should collapse newlines in simple format channel names', () => {
      const formatter = createChannelsListFormatter('simple');
      formatter.format({ channels: channelsWithNewline });

      const output = logSpy.mock.calls[0][0] as string;
      expect(output).not.toContain('\n');
      expect(output).toContain('general fake-row');
    });
  });
});
