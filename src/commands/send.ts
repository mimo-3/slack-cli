import chalk from 'chalk';
import { Command } from 'commander';
import * as fs from 'fs/promises';
import type { Block, KnownBlock } from '@slack/types';
import { SendOptions } from '../types/commands';
import { createSlackClient } from '../utils/client-factory';
import { wrapCommand } from '../utils/command-wrapper';
import { ERROR_MESSAGES, SUCCESS_MESSAGES } from '../utils/constants';
import { extractErrorMessage } from '../utils/error-utils';
import { FileError } from '../utils/errors';
import { parseProfile } from '../utils/option-parsers';
import { resolvePostAt } from '../utils/schedule-utils';
import { createValidationHook, optionValidators } from '../utils/validators';

export function setupSendCommand(): Command {
  const sendCommand = new Command('send')
    .description('Send or schedule a message to a Slack channel or DM')
    .option('-c, --channel <channel>', 'Target channel name or ID')
    .option('--user <username>', 'Send DM to user by username')
    .option('--email <email>', 'Send DM to user by email address')
    .option('-m, --message <message>', 'Message to send')
    .option('-f, --file <file>', 'File containing message content')
    .option('-b, --blocks <json>', 'Block Kit JSON array string')
    .option('--blocks-file <file>', 'File containing Block Kit JSON array')
    .option('-t, --thread <thread>', 'Thread timestamp to reply to')
    .option('--at <time>', 'Schedule time (Unix timestamp in seconds or ISO 8601)')
    .option('--after <minutes>', 'Schedule message after N minutes')
    .option('--profile <profile>', 'Use specific workspace profile')
    .hook(
      'preAction',
      createValidationHook([
        optionValidators.sendTarget,
        optionValidators.messageOrFile,
        optionValidators.blocksOption,
        optionValidators.threadTimestamp,
        optionValidators.scheduleTiming,
      ])
    )
    .action(
      wrapCommand(async (options: SendOptions) => {
        // Get message content
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

        // Parse blocks
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
              ERROR_MESSAGES.BLOCKS_FILE_READ_ERROR(
                options.blocksFile,
                extractErrorMessage(error)
              )
            );
          }
        } else if (options.blocks) {
          blocks = JSON.parse(options.blocks);
        }

        const postAt = resolvePostAt(options.at, options.after);

        // Send message
        const profile = parseProfile(options.profile);
        const client = await createSlackClient(profile);

        // Resolve target channel
        let targetChannel: string;
        let targetLabel: string;

        if (options.user) {
          const userId = await client.resolveUserIdByName(options.user);
          targetChannel = await client.openDmChannel(userId);
          targetLabel = `@${options.user.replace(/^@/, '')}`;
        } else if (options.email) {
          const user = await client.lookupUserByEmail(options.email);
          targetChannel = await client.openDmChannel(user.id!);
          targetLabel = options.email;
        } else {
          targetChannel = options.channel!;
          targetLabel = `#${options.channel}`;
        }

        if (postAt !== null) {
          await client.scheduleMessage(
            targetChannel,
            messageContent,
            postAt,
            options.thread,
            blocks
          );
          const postAtIso = new Date(postAt * 1000).toISOString();
          if (options.user || options.email) {
            console.log(chalk.green(`✓ Message scheduled to ${targetLabel} at ${postAtIso}`));
          } else {
            console.log(
              chalk.green(`✓ ${SUCCESS_MESSAGES.MESSAGE_SCHEDULED(options.channel!, postAtIso)}`)
            );
          }
          return;
        }

        await client.sendMessage(targetChannel, messageContent, options.thread, blocks);
        if (options.user || options.email) {
          console.log(chalk.green(`✓ DM sent to ${targetLabel}`));
        } else {
          console.log(chalk.green(`✓ ${SUCCESS_MESSAGES.MESSAGE_SENT(options.channel!)}`));
        }
      })
    );

  return sendCommand;
}
