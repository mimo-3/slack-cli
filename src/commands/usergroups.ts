import { Command } from 'commander';
import { UsergroupsListOptions, UsergroupsMembersOptions } from '../types/commands';
import { SlackUsergroup } from '../types/slack';
import { renderByFormat, withSlackClient } from '../utils/command-support';
import { wrapCommand } from '../utils/command-wrapper';
import { sanitizeTerminalData, sanitizeTerminalText } from '../utils/terminal-sanitizer';
import { createValidationHook, optionValidators } from '../utils/validators';

interface UsergroupMemberInfo {
  id: string;
  name?: string;
  real_name?: string;
}

function renderUsergroupTable(usergroups: SlackUsergroup[]) {
  const rows = usergroups.map((usergroup) => ({
    id: sanitizeTerminalText(usergroup.id || ''),
    handle: sanitizeTerminalText(usergroup.handle || ''),
    name: sanitizeTerminalText(usergroup.name || ''),
    description: sanitizeTerminalText(usergroup.description || ''),
    user_count: usergroup.user_count ?? '',
  }));

  console.table(sanitizeTerminalData(rows));
}

function renderUsergroupSimple(usergroups: SlackUsergroup[]) {
  for (const usergroup of usergroups) {
    console.log(
      `${sanitizeTerminalText(usergroup.id || '')}\t@${sanitizeTerminalText(
        usergroup.handle || ''
      )}\t${sanitizeTerminalText(usergroup.name || '')}`
    );
  }
}

function renderMemberTable(members: UsergroupMemberInfo[]) {
  const rows = members.map((member) => ({
    id: sanitizeTerminalText(member.id),
    name: sanitizeTerminalText(member.name || ''),
    real_name: sanitizeTerminalText(member.real_name || ''),
  }));

  console.table(sanitizeTerminalData(rows));
}

function renderMemberSimple(members: UsergroupMemberInfo[]) {
  for (const member of members) {
    console.log(
      `${sanitizeTerminalText(member.id)}\t${sanitizeTerminalText(
        member.name || ''
      )}\t${sanitizeTerminalText(member.real_name || '')}`
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

          const members: UsergroupMemberInfo[] = await Promise.all(
            memberIds.map(async (userId) => {
              try {
                const user = await client.getUserInfo(userId);
                return {
                  id: userId,
                  name: user.name,
                  real_name: user.real_name,
                };
              } catch {
                return { id: userId };
              }
            })
          );

          renderByFormat(options, members, {
            table: renderMemberTable,
            simple: renderMemberSimple,
          });
        });
      })
    );

  usergroupsCommand.addCommand(listCommand);
  usergroupsCommand.addCommand(membersCommand);

  return usergroupsCommand;
}
