import chalk from 'chalk';
import { Command } from 'commander';
import * as fs from 'fs/promises';
import type { DraftListOptions, DraftSaveOptions, DraftShowOptions } from '../types/commands';
import { wrapCommand } from '../utils/command-wrapper';
import { ERROR_MESSAGES } from '../utils/constants';
import { DraftStorage } from '../utils/draft-storage';
import { extractErrorMessage } from '../utils/error-utils';
import { FileError, ValidationError } from '../utils/errors';
import {
  createDraftListFormatter,
  createDraftShowFormatter,
} from '../utils/formatters/draft-formatters';
import { parseFormat, parseProfile } from '../utils/option-parsers';
import { createValidationHook, optionValidators } from '../utils/validators';

const DRAFT_DEFAULT_PROFILE = 'default';

export function setupDraftCommand(): Command {
  const draftCommand = new Command('draft').description(
    'Manage local message drafts (送信されない)'
  );

  draftCommand.addCommand(buildSaveCommand());
  draftCommand.addCommand(buildListCommand());
  draftCommand.addCommand(buildShowCommand());
  draftCommand.addCommand(buildDeleteCommand());

  return draftCommand;
}

function buildSaveCommand(): Command {
  return new Command('save')
    .description('Save a new draft, or update an existing one with --id')
    .requiredOption('-c, --channel <channel>', 'Target channel name or ID')
    .option('--channel-label <label>', 'Human-readable channel label (for display)')
    .option('-t, --thread <thread>', 'Thread timestamp to reply to')
    .option('-m, --message <message>', 'Message body')
    .option('-f, --file <file>', 'File containing message body')
    .option('-b, --blocks <json>', 'Block Kit JSON array string')
    .option('--blocks-file <file>', 'File containing Block Kit JSON array')
    .option('--note <note>', 'Free-form note (e.g., context for the reviewer)')
    .option('--id <id>', 'Update the draft with this id instead of creating a new one')
    .option('--format <format>', 'Output format for the saved draft: table, simple, json')
    .option('--profile <profile>', 'Use specific workspace profile')
    .hook(
      'preAction',
      createValidationHook([
        optionValidators.blocksOption,
        optionValidators.threadTimestamp,
        optionValidators.format,
      ])
    )
    .action(
      wrapCommand(async (options: DraftSaveOptions) => {
        const hasBlocks = Boolean(options.blocks || options.blocksFile);
        if (!options.message && !options.file && !hasBlocks) {
          throw new ValidationError(ERROR_MESSAGES.NO_MESSAGE_OR_FILE);
        }
        if (options.message && options.file) {
          throw new ValidationError(ERROR_MESSAGES.BOTH_MESSAGE_AND_FILE);
        }

        let message: string | undefined;
        if (options.file) {
          try {
            message = await fs.readFile(options.file, 'utf-8');
          } catch (error) {
            throw new FileError(
              ERROR_MESSAGES.FILE_READ_ERROR(options.file, extractErrorMessage(error))
            );
          }
        } else if (options.message !== undefined) {
          message = options.message;
        }

        let blocks: unknown[] | undefined;
        if (options.blocksFile) {
          try {
            const blocksJson = await fs.readFile(options.blocksFile, 'utf-8');
            const parsed = JSON.parse(blocksJson);
            if (!Array.isArray(parsed)) {
              throw new Error('blocks must be a JSON array');
            }
            blocks = parsed;
          } catch (error) {
            if (error instanceof SyntaxError) {
              throw new FileError(ERROR_MESSAGES.INVALID_BLOCKS_JSON);
            }
            throw new FileError(
              ERROR_MESSAGES.BLOCKS_FILE_READ_ERROR(options.blocksFile, extractErrorMessage(error))
            );
          }
        } else if (options.blocks) {
          blocks = JSON.parse(options.blocks);
        }

        const profile = parseProfile(options.profile) ?? DRAFT_DEFAULT_PROFILE;
        const storage = new DraftStorage();

        const saved = await storage.save({
          id: options.id,
          channel: options.channel,
          channelLabel: options.channelLabel,
          thread: options.thread,
          message,
          blocks,
          note: options.note,
          profile,
        });

        if (options.format === 'json') {
          console.log(JSON.stringify(saved, null, 2));
          return;
        }

        const verb = options.id ? 'Updated' : 'Saved';
        console.log(chalk.green(`✓ ${verb} draft ${saved.id}`));
      })
    );
}

function buildListCommand(): Command {
  return new Command('list')
    .description('List saved drafts')
    .option('--format <format>', 'Output format: table, simple, json', 'table')
    .option('--profile <profile>', 'Use specific workspace profile')
    .hook('preAction', createValidationHook([optionValidators.format]))
    .action(
      wrapCommand(async (options: DraftListOptions) => {
        const storage = new DraftStorage();
        const drafts = await storage.list();

        if (drafts.length === 0) {
          console.log('No drafts found');
          return;
        }

        const format = parseFormat(options.format);
        const formatter = createDraftListFormatter(format);
        formatter.format({ drafts });
      })
    );
}

async function runArgCommand(fn: () => Promise<void> | void): Promise<void> {
  try {
    await fn();
  } catch (error) {
    console.error(chalk.red('✗ Error:'), extractErrorMessage(error));
    if (process.env.NODE_ENV === 'development' && error instanceof Error) {
      console.error(chalk.gray(error.stack));
    }
    process.exit(1);
  }
}

function buildShowCommand(): Command {
  return new Command('show')
    .description('Show the contents of a draft')
    .argument('<id>', 'Draft id')
    .option('--format <format>', 'Output format: table, simple, json', 'table')
    .hook('preAction', createValidationHook([optionValidators.format]))
    .action(async (id: string, options: DraftShowOptions) => {
      await runArgCommand(async () => {
        const storage = new DraftStorage();
        const draft = await storage.get(id);
        if (!draft) {
          throw new Error(`Draft not found: ${id}`);
        }

        const format = parseFormat(options.format);
        const formatter = createDraftShowFormatter(format);
        formatter.format({ draft });
      });
    });
}

function buildDeleteCommand(): Command {
  return new Command('delete')
    .description('Delete a draft')
    .argument('<id>', 'Draft id')
    .action(async (id: string) => {
      await runArgCommand(async () => {
        const storage = new DraftStorage();
        await storage.delete(id);
        console.log(chalk.green(`✓ Deleted draft ${id}`));
      });
    });
}
