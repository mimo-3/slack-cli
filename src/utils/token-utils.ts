import { TOKEN_MASK_LENGTH, TOKEN_MIN_LENGTH } from './constants';

// Matches any Slack token-shaped substring (xoxb/xoxp/xoxa/xoxr/xoxs/xoxo prefix
// followed by an alphanumeric body with optional dashes). Used to redact tokens
// that may leak into stack traces or other debug output.
const SLACK_TOKEN_PATTERN = /xox[bpoars]-[A-Za-z0-9-]+/gi;

export function redactSlackTokens(text: string): string;
export function redactSlackTokens(text: undefined): undefined;
export function redactSlackTokens(text: string | undefined): string | undefined;
export function redactSlackTokens(text: string | undefined): string | undefined {
  if (text === undefined) {
    return undefined;
  }
  return text.replace(SLACK_TOKEN_PATTERN, (match) => {
    const prefix = match.slice(0, 4).toLowerCase();
    return `${prefix}-***-REDACTED`;
  });
}

/**
 * Masks a token for display purposes, showing only first and last few characters
 * @param token The token to mask
 * @returns Masked token in format "xoxb-****-****-abcd"
 */
export function maskToken(token: string): string {
  if (token.length <= TOKEN_MIN_LENGTH) {
    return '****';
  }

  const prefix = token.substring(0, TOKEN_MASK_LENGTH);
  const suffix = token.substring(token.length - TOKEN_MASK_LENGTH);

  return `${prefix}-****-****-${suffix}`;
}
