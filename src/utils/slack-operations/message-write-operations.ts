import type { Block, KnownBlock } from '@slack/types';
import {
  ChatPostEphemeralResponse,
  ChatPostMessageResponse,
  ChatScheduleMessageResponse,
  ChatUpdateResponse,
} from '@slack/web-api';
import type { ScheduledMessage } from '../../types/slack';
import { BaseSlackClient, SlackClientDependency } from './base-client';
import { ChannelOperations } from './channel-operations';

/** @internal Internal split of MessageOperations write responsibilities. */
export class MessageWriteOperations extends BaseSlackClient {
  private channelOps: ChannelOperations;

  constructor(dependency: SlackClientDependency, channelOps?: ChannelOperations) {
    super(dependency);
    this.channelOps = channelOps ?? new ChannelOperations(dependency);
  }

  async sendMessage(
    channel: string,
    text: string,
    thread_ts?: string,
    blocks?: (KnownBlock | Block)[]
  ): Promise<ChatPostMessageResponse> {
    return await this.client.chat.postMessage({
      channel,
      text,
      ...(thread_ts ? { thread_ts } : {}),
      ...(blocks ? { blocks } : {}),
    });
  }

  async sendEphemeralMessage(
    channel: string,
    user: string,
    text: string,
    thread_ts?: string,
    blocks?: (KnownBlock | Block)[]
  ): Promise<ChatPostEphemeralResponse> {
    return await this.client.chat.postEphemeral({
      channel,
      user,
      text,
      ...(thread_ts ? { thread_ts } : {}),
      ...(blocks ? { blocks } : {}),
    });
  }

  async scheduleMessage(
    channel: string,
    text: string,
    post_at: number,
    thread_ts?: string,
    blocks?: (KnownBlock | Block)[]
  ): Promise<ChatScheduleMessageResponse> {
    return await this.client.chat.scheduleMessage({
      channel,
      text,
      post_at,
      ...(thread_ts ? { thread_ts } : {}),
      ...(blocks ? { blocks } : {}),
    });
  }

  async listScheduledMessages(channel?: string, limit = 50): Promise<ScheduledMessage[]> {
    const channelId = channel ? await this.channelOps.resolveChannelId(channel) : undefined;
    const response = await this.client.chat.scheduledMessages.list({
      limit,
      ...(channelId ? { channel: channelId } : {}),
    });

    return (response.scheduled_messages || []) as ScheduledMessage[];
  }

  async updateMessage(
    channel: string,
    ts: string,
    text: string,
    blocks?: (KnownBlock | Block)[]
  ): Promise<ChatUpdateResponse> {
    const channelId = await this.channelOps.resolveChannelId(channel);

    return await this.client.chat.update({
      channel: channelId,
      ts,
      text,
      ...(blocks ? { blocks } : {}),
    });
  }

  async deleteMessage(channel: string, ts: string): Promise<void> {
    const channelId = await this.channelOps.resolveChannelId(channel);

    await this.client.chat.delete({
      channel: channelId,
      ts,
    });
  }

  async cancelScheduledMessage(channel: string, scheduledMessageId: string): Promise<void> {
    const channelId = await this.channelOps.resolveChannelId(channel);

    await this.client.chat.deleteScheduledMessage({
      channel: channelId,
      scheduled_message_id: scheduledMessageId,
    });
  }
}
