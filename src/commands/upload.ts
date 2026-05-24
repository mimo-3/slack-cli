import chalk from 'chalk';
import { Command } from 'commander';
import * as fs from 'fs/promises';
import { UploadOptions } from '../types/commands';
import { createSlackClient } from '../utils/client-factory';
import { renderByFormat } from '../utils/command-support';
import { wrapCommand } from '../utils/command-wrapper';
import { FileError } from '../utils/errors';
import { parseProfile } from '../utils/option-parsers';
import type { UploadedFileInfo } from '../utils/slack-operations/file-operations';
import { createValidationHook, optionValidators } from '../utils/validators';

interface UploadOutput {
  channel: string;
  files: UploadedFileInfo[];
}

export function setupUploadCommand(): Command {
  const uploadCommand = new Command('upload')
    .description('Upload a file or snippet to a Slack channel')
    .requiredOption('-c, --channel <channel>', 'Channel name or ID')
    .option('-f, --file <file>', 'File path to upload')
    .option('--content <content>', 'Text content to upload as snippet')
    .option('--filename <filename>', 'Override filename')
    .option('--title <title>', 'File title')
    .option('-m, --message <message>', 'Initial comment with the file')
    .option('--filetype <filetype>', 'Snippet type (e.g. python, javascript, csv)')
    .option('-t, --thread <thread>', 'Thread timestamp to upload as reply')
    .option('--format <format>', 'Output format: table, simple, json', 'table')
    .option('--profile <profile>', 'Use specific workspace profile')
    .hook(
      'preAction',
      createValidationHook([
        optionValidators.fileOrContent,
        optionValidators.uploadThreadTimestamp,
        optionValidators.format,
      ])
    )
    .action(
      wrapCommand(async (options: UploadOptions) => {
        // Verify file exists if file path provided
        if (options.file) {
          try {
            await fs.access(options.file);
          } catch {
            throw new FileError(`File not found: ${options.file}`);
          }
        }

        const profile = parseProfile(options.profile);
        const client = await createSlackClient(profile);

        const result = options.file
          ? await client.uploadFile({
              channel: options.channel,
              filePath: options.file,
              title: options.title,
              initialComment: options.message,
              snippetType: options.filetype,
              threadTs: options.thread,
              filename: options.filename,
            })
          : await client.uploadFile({
              channel: options.channel,
              content: options.content!,
              title: options.title,
              initialComment: options.message,
              snippetType: options.filetype,
              threadTs: options.thread,
              filename: options.filename,
            });

        const output: UploadOutput = {
          channel: options.channel,
          files: result.files,
        };

        renderByFormat<UploadOutput>(options, output, {
          table: (data) => {
            console.log(chalk.green(`✓ File uploaded successfully to #${data.channel}`));
            for (const f of data.files) {
              if (f.id) console.log(`  file_id: ${f.id}`);
              if (f.permalink) console.log(`  permalink: ${f.permalink}`);
            }
          },
        });
      })
    );

  return uploadCommand;
}
