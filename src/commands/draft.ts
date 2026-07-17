import chalk from 'chalk';
import { Command } from 'commander';
import { renderByFormat, withSlackClient } from '../utils/command-support';
import { wrapCommand } from '../utils/command-wrapper';
import { type Draft, DraftStore } from '../utils/draft-store';
import { ValidationError } from '../utils/errors';
import { sanitizeTerminalData, sanitizeTerminalText } from '../utils/terminal-sanitizer';
import { createValidationHook, optionValidators } from '../utils/validators';

interface DraftSaveOptions {
  channel?: string;
  user?: string;
  message?: string;
  thread?: string;
}

interface DraftListOptions {
  format?: string;
}

interface DraftIdOptions {
  id: string;
}

interface DraftSendOptions extends DraftIdOptions {
  keep?: boolean;
  profile?: string;
}

function formatTarget(draft: Draft): string {
  return draft.user ? `@${draft.user}` : `#${draft.channel}`;
}

function renderTable(drafts: Draft[]) {
  const rows = drafts.map((draft) => ({
    id: sanitizeTerminalText(draft.id),
    target: sanitizeTerminalText(formatTarget(draft)),
    created_at: draft.createdAt,
    message: sanitizeTerminalText(
      draft.message.length > 60 ? `${draft.message.slice(0, 60)}...` : draft.message
    ),
  }));

  console.table(sanitizeTerminalData(rows));
}

function renderSimple(drafts: Draft[]) {
  for (const draft of drafts) {
    console.log(
      `${sanitizeTerminalText(draft.id)} ${draft.createdAt} ${sanitizeTerminalText(formatTarget(draft))} ${sanitizeTerminalText(draft.message)}`
    );
  }
}

export function setupDraftCommand(): Command {
  const draftCommand = new Command('draft').description(
    'Manage message drafts (save, list, show, send, delete)'
  );

  const saveCommand = new Command('save')
    .description('Save a message as a local draft')
    .option('-c, --channel <channel>', 'Target channel name or ID')
    .option('--user <username>', 'Target user for DM')
    .option('-m, --message <message>', 'Message content')
    .option('-t, --thread <thread>', 'Thread timestamp to reply to')
    .hook('preAction', createValidationHook([optionValidators.threadTimestamp]))
    .action(
      wrapCommand(async (options: DraftSaveOptions) => {
        if (!options.channel && !options.user) {
          throw new ValidationError('Either --channel or --user must be specified');
        }
        if (options.channel && options.user) {
          throw new ValidationError('Cannot specify both --channel and --user');
        }
        if (!options.message) {
          throw new ValidationError('--message is required');
        }

        const store = new DraftStore();
        const draft = await store.save({
          channel: options.channel,
          user: options.user,
          message: options.message,
          thread: options.thread,
        });

        console.log(chalk.green(`✓ Draft saved (id: ${draft.id}, target: ${formatTarget(draft)})`));
      })
    );

  const listCommand = new Command('list')
    .description('List saved drafts')
    .option('--format <format>', 'Output format: table, simple, json', 'table')
    .hook('preAction', createValidationHook([optionValidators.format]))
    .action(
      wrapCommand(async (options: DraftListOptions) => {
        const store = new DraftStore();
        const drafts = await store.list();

        if (drafts.length === 0) {
          console.log('No drafts found');
          return;
        }

        renderByFormat(options, drafts, {
          table: renderTable,
          simple: renderSimple,
        });
      })
    );

  const showCommand = new Command('show')
    .description('Show the full content of a draft')
    .requiredOption('--id <draftId>', 'Draft ID')
    .action(
      wrapCommand(async (options: DraftIdOptions) => {
        const store = new DraftStore();
        const draft = await store.get(options.id);
        if (!draft) {
          throw new ValidationError(`Draft not found: ${options.id}`);
        }

        console.log(`id: ${sanitizeTerminalText(draft.id)}`);
        console.log(`target: ${sanitizeTerminalText(formatTarget(draft))}`);
        if (draft.thread) {
          console.log(`thread: ${sanitizeTerminalText(draft.thread)}`);
        }
        console.log(`created_at: ${draft.createdAt}`);
        console.log('---');
        console.log(sanitizeTerminalText(draft.message));
      })
    );

  const sendCommand = new Command('send')
    .description('Send a saved draft')
    .requiredOption('--id <draftId>', 'Draft ID')
    .option('--keep', 'Keep the draft after sending')
    .option('--profile <profile>', 'Use specific workspace profile')
    .action(
      wrapCommand(async (options: DraftSendOptions) => {
        const store = new DraftStore();
        const draft = await store.get(options.id);
        if (!draft) {
          throw new ValidationError(`Draft not found: ${options.id}`);
        }

        await withSlackClient(options, async (client) => {
          let targetChannel: string;
          if (draft.user) {
            const userId = await client.resolveUserIdByName(draft.user);
            targetChannel = await client.openDmChannel(userId);
          } else {
            targetChannel = draft.channel!;
          }

          await client.sendMessage(targetChannel, draft.message, draft.thread);
        });

        console.log(chalk.green(`✓ Draft sent to ${formatTarget(draft)}`));

        if (!options.keep) {
          await store.delete(draft.id);
          console.log(chalk.gray(`Draft ${draft.id} deleted`));
        }
      })
    );

  const deleteCommand = new Command('delete')
    .description('Delete a saved draft')
    .requiredOption('--id <draftId>', 'Draft ID')
    .action(
      wrapCommand(async (options: DraftIdOptions) => {
        const store = new DraftStore();
        await store.delete(options.id);
        console.log(chalk.green(`✓ Draft ${options.id} deleted`));
      })
    );

  draftCommand.addCommand(saveCommand);
  draftCommand.addCommand(listCommand);
  draftCommand.addCommand(showCommand);
  draftCommand.addCommand(sendCommand);
  draftCommand.addCommand(deleteCommand);

  return draftCommand;
}
