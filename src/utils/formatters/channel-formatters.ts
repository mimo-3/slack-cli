import chalk from 'chalk';
import { Channel } from '../../types/slack';
import { formatChannelName } from '../channel-formatter';
import { formatSlackTimestamp } from '../date-utils';
import { sanitizeSingleLineText } from '../terminal-sanitizer';
import { AbstractFormatter, createFormatterFactory, JsonFormatter } from './base-formatter';

function singleLineChannelName(channel: Channel): string {
  return sanitizeSingleLineText(channel.display_name || formatChannelName(channel.name));
}

export interface ChannelFormatterOptions {
  channels: Channel[];
  countOnly?: boolean;
}

class ChannelTableFormatter extends AbstractFormatter<ChannelFormatterOptions> {
  format({ channels }: ChannelFormatterOptions): void {
    console.log(chalk.bold('Channel          Unread  Last Message'));
    console.log('─'.repeat(50));

    channels.forEach((channel) => {
      const paddedName = singleLineChannelName(channel).padEnd(16);
      const count = (channel.unread_count || 0).toString().padEnd(6);
      const lastRead = channel.last_read ? formatSlackTimestamp(channel.last_read) : 'Unknown';
      console.log(`${paddedName} ${count}  ${lastRead}`);
    });
  }
}

class ChannelSimpleFormatter extends AbstractFormatter<ChannelFormatterOptions> {
  format({ channels }: ChannelFormatterOptions): void {
    channels.forEach((channel) => {
      console.log(`${singleLineChannelName(channel)} (${channel.unread_count || 0})`);
    });
  }
}

class ChannelJsonFormatter extends JsonFormatter<ChannelFormatterOptions> {
  protected transform({ channels }: ChannelFormatterOptions) {
    return channels.map((channel) => ({
      channel: channel.display_name || formatChannelName(channel.name),
      channelId: channel.id,
      unreadCount: channel.unread_count || 0,
    }));
  }
}

class ChannelCountFormatter extends AbstractFormatter<ChannelFormatterOptions> {
  format({ channels }: ChannelFormatterOptions): void {
    let totalUnread = 0;
    channels.forEach((channel) => {
      const count = channel.unread_count || 0;
      totalUnread += count;
      console.log(`${singleLineChannelName(channel)}: ${count}`);
    });
    console.log(chalk.bold(`Total: ${totalUnread} unread messages`));
  }
}

const channelFormatterFactory = createFormatterFactory<ChannelFormatterOptions>({
  table: new ChannelTableFormatter(),
  simple: new ChannelSimpleFormatter(),
  json: new ChannelJsonFormatter(),
  count: new ChannelCountFormatter(),
});

export function createChannelFormatter(format: string, countOnly: boolean) {
  if (countOnly) {
    return channelFormatterFactory.create('count');
  }
  return channelFormatterFactory.create(format);
}
