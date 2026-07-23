import type { SlackUsergroup } from '../../types/slack';
import { ApiError } from '../errors';
import { BaseSlackClient, SlackClientDependency } from './base-client';

export class UsergroupOperations extends BaseSlackClient {
  constructor(dependency: SlackClientDependency) {
    super(dependency);
  }

  async listUsergroups(includeDisabled = false): Promise<SlackUsergroup[]> {
    const response = await this.client.usergroups.list({
      include_count: true,
      ...(includeDisabled ? { include_disabled: true } : {}),
    });

    return (response.usergroups || []) as SlackUsergroup[];
  }

  async listUsergroupUsers(usergroupId: string): Promise<string[]> {
    const response = await this.client.usergroups.users.list({
      usergroup: usergroupId,
    });

    return (response.users || []) as string[];
  }

  async resolveUsergroupIdByHandle(handle: string): Promise<string> {
    const normalized = handle.replace(/^@/, '').toLowerCase();

    const usergroups = await this.listUsergroups(true);
    for (const usergroup of usergroups) {
      if (usergroup.handle?.toLowerCase() === normalized) {
        return usergroup.id!;
      }
    }

    throw new ApiError(`Usergroup '@${handle.replace(/^@/, '')}' not found`);
  }
}
