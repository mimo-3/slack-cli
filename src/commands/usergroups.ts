import { Command } from 'commander';
import { UsergroupsListOptions, UsergroupsMembersOptions } from '../types/commands';
import { SlackUsergroup } from '../types/slack';
import { renderByFormat, withSlackClient } from '../utils/command-support';
import { wrapCommand } from '../utils/command-wrapper';
import { createMembersFormatter, MemberInfo } from '../utils/formatters/members-formatters';
import { parseFormat } from '../utils/option-parsers';
import { sanitizeSingleLineText, sanitizeTerminalData } from '../utils/terminal-sanitizer';
import { createValidationHook, optionValidators } from '../utils/validators';

function renderUsergroupTable(usergroups: SlackUsergroup[]) {
  const rows = usergroups.map((usergroup) => ({
    id: sanitizeSingleLineText(usergroup.id || ''),
    handle: sanitizeSingleLineText(usergroup.handle || ''),
    name: sanitizeSingleLineText(usergroup.name || ''),
    description: sanitizeSingleLineText(usergroup.description || ''),
    user_count: usergroup.user_count ?? '',
  }));

  console.table(sanitizeTerminalData(rows));
}

function renderUsergroupSimple(usergroups: SlackUsergroup[]) {
  for (const usergroup of usergroups) {
    console.log(
      `${sanitizeSingleLineText(usergroup.id || '')}\t@${sanitizeSingleLineText(
        usergroup.handle || ''
      )}\t${sanitizeSingleLineText(usergroup.name || '')}`
    );
  }
}

export function setupUsergroupsCommand(): Command {
  const usergroupsCommand = new Command('usergroups').description(
    'List user groups and their members'
  );

  const listCommand = new Command('list')
    .description('List user groups in the workspace')
    .option('--include-disabled', 'Include disabled user groups')
    .option('--format <format>', 'Output format: table, simple, json', 'table')
    .option('--profile <profile>', 'Use specific workspace profile')
    .hook('preAction', createValidationHook([optionValidators.format]))
    .action(
      wrapCommand(async (options: UsergroupsListOptions) => {
        await withSlackClient(options, async (client) => {
          const usergroups = await client.listUsergroups(options.includeDisabled ?? false);

          if (usergroups.length === 0) {
            console.log('No usergroups found');
            return;
          }

          renderByFormat(options, usergroups, {
            table: renderUsergroupTable,
            simple: renderUsergroupSimple,
          });
        });
      })
    );

  const membersCommand = new Command('members')
    .description('List members of a user group')
    .option('--id <usergroupId>', 'User group ID')
    .option('--handle <handle>', 'User group handle (e.g. @engineers)')
    .option('--format <format>', 'Output format: table, simple, json', 'table')
    .option('--profile <profile>', 'Use specific workspace profile')
    .hook('preAction', createValidationHook([optionValidators.format]))
    .action(
      wrapCommand(async (options: UsergroupsMembersOptions) => {
        if (!options.id && !options.handle) {
          throw new Error('You must specify either --id or --handle');
        }
        if (options.id && options.handle) {
          throw new Error('Cannot use both --id and --handle');
        }

        await withSlackClient(options, async (client) => {
          let usergroupId: string;
          if (options.handle) {
            usergroupId = await client.resolveUsergroupIdByHandle(options.handle);
          } else {
            usergroupId = options.id!;
          }

          const memberIds = await client.listUsergroupMembers(usergroupId);

          if (memberIds.length === 0) {
            console.log('No members found');
            return;
          }

          const members: MemberInfo[] = await Promise.all(
            memberIds.map(async (userId) => {
              try {
                const user = await client.getUserInfo(userId);
                return {
                  id: userId,
                  name: user.name,
                  realName: user.real_name,
                };
              } catch {
                return { id: userId };
              }
            })
          );

          const format = parseFormat(options.format);
          createMembersFormatter(format).format({ members });
        });
      })
    );

  usergroupsCommand.addCommand(listCommand);
  usergroupsCommand.addCommand(membersCommand);

  return usergroupsCommand;
}
