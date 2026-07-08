import chalk from 'chalk';
import { Command } from 'commander';
import { DownloadOptions } from '../types/commands';
import { createSlackClient } from '../utils/client-factory';
import { renderByFormat } from '../utils/command-support';
import { wrapCommand } from '../utils/command-wrapper';
import { parseProfile } from '../utils/option-parsers';
import type { DownloadFileResult } from '../utils/slack-operations/file-operations';
import { createValidationHook } from '../utils/validators';

function formatFileSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

export function setupDownloadCommand(): Command {
  const downloadCommand = new Command('download')
    .description('Download a file from Slack')
    .option('-u, --url <url>', 'File URL (url_private or url_private_download from message)')
    .option('-i, --id <id>', 'Slack file ID (e.g. F0BFXAEP1UZ)')
    .option(
      '-o, --output <path>',
      'Output file path (defaults to original filename in current dir)'
    )
    .option('--format <format>', 'Output format: table, simple, json', 'table')
    .option('--profile <profile>', 'Use specific workspace profile')
    .hook(
      'preAction',
      createValidationHook([
        (options) => {
          if (!options.url && !options.id) {
            return 'You must specify either --url or --id';
          }
          if (options.url && options.id) {
            return 'Cannot use both --url and --id';
          }
          return null;
        },
      ])
    )
    .action(
      wrapCommand(async (options: DownloadOptions) => {
        const profile = parseProfile(options.profile);
        const client = await createSlackClient(profile);

        const result = await client.downloadFile({
          url: options.url,
          fileId: options.id,
          outputPath: options.output,
        });

        renderByFormat<DownloadFileResult>(options, result, {
          table: (data) => {
            console.log(chalk.green(`✓ Downloaded: ${data.fileName}`));
            console.log(`  path: ${data.filePath}`);
            console.log(`  size: ${formatFileSize(data.size)}`);
          },
          simple: (data) => {
            console.log(data.filePath);
          },
        });
      })
    );

  return downloadCommand;
}
