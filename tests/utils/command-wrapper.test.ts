import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { wrapCommand } from '../../src/utils/command-wrapper';

describe('wrapCommand', () => {
  let errorSpy: ReturnType<typeof vi.spyOn>;
  let exitSpy: ReturnType<typeof vi.spyOn>;

  beforeEach(() => {
    errorSpy = vi.spyOn(console, 'error').mockImplementation(() => {});
    exitSpy = vi.spyOn(process, 'exit').mockImplementation(() => undefined as never);
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it('should run the action and not exit on success', async () => {
    const action = vi.fn().mockResolvedValue(undefined);

    await wrapCommand(action)({});

    expect(action).toHaveBeenCalled();
    expect(exitSpy).not.toHaveBeenCalled();
  });

  it('should print the error message and exit with code 1', async () => {
    const action = vi.fn().mockRejectedValue(new Error('channel_not_found'));

    await wrapCommand(action)({});

    expect(errorSpy).toHaveBeenCalledWith(
      expect.stringContaining('Error:'),
      expect.stringContaining('channel_not_found')
    );
    expect(exitSpy).toHaveBeenCalledWith(1);
  });

  it('should strip ANSI escape sequences from error messages', async () => {
    const action = vi.fn().mockRejectedValue(new Error('bad_request [2J[1;1Hinjected'));

    await wrapCommand(action)({});

    const message = errorSpy.mock.calls[0][1] as string;
    expect(message).not.toContain('');
    expect(message).toContain('bad_request');
    expect(message).toContain('injected');
  });

  it('should strip OSC sequences from error messages', async () => {
    const action = vi.fn().mockRejectedValue(new Error('failed ]0;evil titlerest'));

    await wrapCommand(action)({});

    const message = errorSpy.mock.calls[0][1] as string;
    expect(message).not.toContain('');
    expect(message).not.toContain('evil title');
    expect(message).toContain('failed');
    expect(message).toContain('rest');
  });

  it('should still redact Slack tokens in error messages', async () => {
    const fakeToken = ['xoxb', '1234567890', 'abcdefghij'].join('-');
    const action = vi.fn().mockRejectedValue(new Error(`invalid_auth: ${fakeToken}`));

    await wrapCommand(action)({});

    const message = errorSpy.mock.calls[0][1] as string;
    expect(message).not.toContain(fakeToken);
    expect(message).toContain('invalid_auth');
  });

  it('should redact tokens split by injected escape sequences', async () => {
    const tokenHead = ['xoxb', '12345'].join('-');
    const tokenTail = '67890-abcdefghij';
    const action = vi
      .fn()
      .mockRejectedValue(new Error(`invalid_auth: ${tokenHead}[0m${tokenTail}`));

    await wrapCommand(action)({});

    const message = errorSpy.mock.calls[0][1] as string;
    expect(message).not.toContain(tokenTail);
    expect(message).toContain('invalid_auth');
  });
});
