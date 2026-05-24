import type { Block, KnownBlock } from '@slack/types';
import chalk from 'chalk';
import { Command } from 'commander';
import * as fs from 'fs/promises';
import { EditOptions } from '../types/commands';
import { createSlackClient } from '../utils/client-factory';
import { wrapCommand } from '../utils/command-wrapper';
import { ERROR_MESSAGES } from '../utils/constants';
import { extractErrorMessage } from '../utils/error-utils';
import { FileError } from '../utils/errors';
import { parseProfile } from '../utils/option-parsers';
import { createValidationHook, optionValidators } from '../utils/validators';

export function setupEditCommand(): Command {
  const editCommand = new Command('edit')
    .description('Edit a sent message')
    .requiredOption('-c, --channel <channel>', 'Channel name or ID')
    .requiredOption('--ts <timestamp>', 'Message timestamp to edit')
    .option('-m, --message <message>', 'New message text')
    .option('-f, --file <file>', 'File containing new message text')
    .option('-b, --blocks <json>', 'Block Kit JSON array string')
    .option('--blocks-file <file>', 'File containing Block Kit JSON array')
    .option('--profile <profile>', 'Use specific workspace profile')
    .hook(
      'preAction',
      createValidationHook([
        optionValidators.editTimestamp,
        optionValidators.messageOrFile,
        optionValidators.blocksOption,
      ])
    )
    .action(
      wrapCommand(async (options: EditOptions) => {
        let messageContent: string;
        if (options.file) {
          try {
            messageContent = await fs.readFile(options.file, 'utf-8');
          } catch (error) {
            throw new FileError(
              ERROR_MESSAGES.FILE_READ_ERROR(options.file, extractErrorMessage(error))
            );
          }
        } else {
          messageContent = options.message ?? '';
        }

        let blocks: (KnownBlock | Block)[] | undefined;
        if (options.blocksFile) {
          try {
            const blocksJson = await fs.readFile(options.blocksFile, 'utf-8');
            blocks = JSON.parse(blocksJson);
            if (!Array.isArray(blocks)) {
              throw new Error('blocks must be a JSON array');
            }
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

        const profile = parseProfile(options.profile);
        const client = await createSlackClient(profile);

        if (blocks) {
          await client.updateMessage(options.channel, options.ts, messageContent, blocks);
        } else {
          await client.updateMessage(options.channel, options.ts, messageContent);
        }
        console.log(chalk.green(`✓ Message updated successfully in #${options.channel}`));
      })
    );

  return editCommand;
}
