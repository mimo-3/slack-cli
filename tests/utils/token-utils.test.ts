import { describe, expect, it } from 'vitest';
import { maskToken, redactSlackTokens } from '../../src/utils/token-utils';

describe('token-utils', () => {
  describe('redactSlackTokens', () => {
    // The strings below are assembled at runtime to avoid false-positive
    // secret scanners on this file.
    const p = (kind: string) => ['xo', `x${kind}`].join('');
    const fake = (kind: string, body: string) => `${p(kind)}-${body}`;
    const redactedFor = (kind: string) => `${p(kind)}-***-REDACTED`;

    it('replaces bot tokens with a placeholder', () => {
      const input = `Auth failed for ${fake('b', 'fakeBodyABC')}`;
      expect(redactSlackTokens(input)).toBe(`Auth failed for ${redactedFor('b')}`);
    });

    it('replaces user, admin, and other Slack token prefixes', () => {
      const kinds = ['p', 'a', 'r', 's', 'o'];
      const input = kinds.map((k) => fake(k, 'fakeBody')).join(' / ');
      const expected = kinds.map((k) => redactedFor(k)).join(' / ');
      expect(redactSlackTokens(input)).toBe(expected);
    });

    it('redacts multiple tokens in the same string', () => {
      const stack = [
        'Error: bad token',
        `    at decrypt (${fake('b', 'fakeOne')})`,
        `    at run (${fake('p', 'fakeTwo')})`,
      ].join('\n');
      const redacted = redactSlackTokens(stack);
      expect(redacted).not.toContain('fakeOne');
      expect(redacted).not.toContain('fakeTwo');
      expect(redacted).toContain(redactedFor('b'));
      expect(redacted).toContain(redactedFor('p'));
    });

    it('returns the input unchanged when no token is present', () => {
      const input = 'Error: channel not found';
      expect(redactSlackTokens(input)).toBe(input);
    });

    it('handles undefined input by returning undefined', () => {
      expect(redactSlackTokens(undefined)).toBeUndefined();
    });
  });

  describe('maskToken', () => {
    it('should mask short tokens completely', () => {
      expect(maskToken('short')).toBe('****');
      expect(maskToken('123456789')).toBe('****');
      expect(maskToken('')).toBe('****');
    });

    it('should mask long tokens showing prefix and suffix', () => {
      const token = 'test-1234567890-abcdefghijklmnop';
      expect(maskToken(token)).toBe('test-****-****-mnop');
    });

    it('should handle tokens of exactly minimum length + 1', () => {
      const token = '1234567890'; // 10 characters
      expect(maskToken(token)).toBe('1234-****-****-7890');
    });

    it('should handle various token formats', () => {
      expect(maskToken('test-123456789012345')).toBe('test-****-****-2345');
      expect(maskToken('demo-2-123456789012345')).toBe('demo-****-****-2345');
      expect(maskToken('1234567890123456789012345')).toBe('1234-****-****-2345');
    });

    it('should handle tokens with special characters', () => {
      expect(maskToken('test-token-with-dashes')).toBe('test-****-****-shes');
      expect(maskToken('token_with_underscores')).toBe('toke-****-****-ores');
    });
  });
});
