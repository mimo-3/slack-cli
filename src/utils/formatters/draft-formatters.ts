import type { DraftRecord } from '../draft-storage';
import { sanitizeTerminalText } from '../terminal-sanitizer';
import { AbstractFormatter, createFormatterFactory, JsonFormatter } from './base-formatter';

export interface DraftListFormatterOptions {
  drafts: DraftRecord[];
}

export interface DraftShowFormatterOptions {
  draft: DraftRecord;
}

function truncate(value: string, width: number): string {
  if (value.length <= width) return value;
  return `${value.slice(0, Math.max(0, width - 1))}…`;
}

function channelLabel(draft: DraftRecord): string {
  return draft.channelLabel || draft.channel;
}

function summarizeMessage(draft: DraftRecord): string {
  const body = draft.message || (draft.blocks ? '[blocks]' : '');
  return body.replace(/\s+/g, ' ').trim();
}

class DraftListTableFormatter extends AbstractFormatter<DraftListFormatterOptions> {
  format({ drafts }: DraftListFormatterOptions): void {
    const idWidth = 30;
    const updatedWidth = 22;
    const channelWidth = 24;
    const textWidth = 40;

    const header =
      'ID'.padEnd(idWidth) +
      'Updated'.padEnd(updatedWidth) +
      'Channel'.padEnd(channelWidth) +
      'Message'.padEnd(textWidth);
    console.log(header);
    console.log('\u2500'.repeat(idWidth + updatedWidth + channelWidth + textWidth));

    drafts.forEach((draft) => {
      const id = sanitizeTerminalText(draft.id).padEnd(idWidth);
      const updated = sanitizeTerminalText(draft.updatedAt).padEnd(updatedWidth);
      const channel = truncate(sanitizeTerminalText(channelLabel(draft)), channelWidth - 2).padEnd(
        channelWidth
      );
      const message = truncate(sanitizeTerminalText(summarizeMessage(draft)), textWidth - 2).padEnd(
        textWidth
      );
      console.log(`${id}${updated}${channel}${message}`);
    });
  }
}

class DraftListSimpleFormatter extends AbstractFormatter<DraftListFormatterOptions> {
  format({ drafts }: DraftListFormatterOptions): void {
    drafts.forEach((draft) => {
      console.log(
        `${sanitizeTerminalText(draft.id)}\t${sanitizeTerminalText(draft.updatedAt)}\t${sanitizeTerminalText(
          channelLabel(draft)
        )}\t${sanitizeTerminalText(summarizeMessage(draft))}`
      );
    });
  }
}

class DraftListJsonFormatter extends JsonFormatter<DraftListFormatterOptions> {
  protected transform({ drafts }: DraftListFormatterOptions) {
    return drafts;
  }
}

const draftListFactory = createFormatterFactory<DraftListFormatterOptions>({
  table: new DraftListTableFormatter(),
  simple: new DraftListSimpleFormatter(),
  json: new DraftListJsonFormatter(),
});

export function createDraftListFormatter(format: string) {
  return draftListFactory.create(format);
}

class DraftShowTableFormatter extends AbstractFormatter<DraftShowFormatterOptions> {
  format({ draft }: DraftShowFormatterOptions): void {
    console.log(`ID:         ${sanitizeTerminalText(draft.id)}`);
    console.log(`Profile:    ${sanitizeTerminalText(draft.profile)}`);
    console.log(`Channel:    ${sanitizeTerminalText(channelLabel(draft))}`);
    if (draft.channelLabel && draft.channelLabel !== draft.channel) {
      console.log(`ChannelID:  ${sanitizeTerminalText(draft.channel)}`);
    }
    if (draft.thread) {
      console.log(`Thread:     ${sanitizeTerminalText(draft.thread)}`);
    }
    if (draft.note) {
      console.log(`Note:       ${sanitizeTerminalText(draft.note)}`);
    }
    console.log(`Created:    ${sanitizeTerminalText(draft.createdAt)}`);
    console.log(`Updated:    ${sanitizeTerminalText(draft.updatedAt)}`);
    console.log('---');
    if (draft.message) {
      console.log(sanitizeTerminalText(draft.message));
    }
    if (draft.blocks) {
      console.log('---');
      console.log('Blocks:');
      console.log(JSON.stringify(draft.blocks, null, 2));
    }
  }
}

class DraftShowSimpleFormatter extends AbstractFormatter<DraftShowFormatterOptions> {
  format({ draft }: DraftShowFormatterOptions): void {
    console.log(
      `${sanitizeTerminalText(draft.id)}\t${sanitizeTerminalText(channelLabel(draft))}\t${sanitizeTerminalText(
        summarizeMessage(draft)
      )}`
    );
  }
}

class DraftShowJsonFormatter extends JsonFormatter<DraftShowFormatterOptions> {
  protected transform({ draft }: DraftShowFormatterOptions) {
    return draft;
  }
}

const draftShowFactory = createFormatterFactory<DraftShowFormatterOptions>({
  table: new DraftShowTableFormatter(),
  simple: new DraftShowSimpleFormatter(),
  json: new DraftShowJsonFormatter(),
});

export function createDraftShowFormatter(format: string) {
  return draftShowFactory.create(format);
}
