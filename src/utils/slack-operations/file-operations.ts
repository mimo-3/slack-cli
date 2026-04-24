import { basename } from 'path';
import { BaseSlackClient, SlackClientDependency } from './base-client';
import { ChannelOperations } from './channel-operations';

export interface UploadFileOptions {
  channel: string;
  filePath?: string;
  content?: string;
  filename?: string;
  title?: string;
  initialComment?: string;
  snippetType?: string;
  threadTs?: string;
}

export interface UploadedFileInfo {
  id?: string;
  name?: string;
  title?: string;
  permalink?: string;
  permalink_public?: string;
  url_private?: string;
}

export interface UploadFileResult {
  ok?: boolean;
  files: UploadedFileInfo[];
}

export class FileOperations extends BaseSlackClient {
  private channelOps: ChannelOperations;

  constructor(dependency: SlackClientDependency, channelOps?: ChannelOperations) {
    super(dependency);
    this.channelOps = channelOps ?? new ChannelOperations(dependency);
  }

  async uploadFile(options: UploadFileOptions): Promise<UploadFileResult> {
    const channelId = await this.channelOps.resolveChannelId(options.channel);

    const params: Record<string, unknown> = {
      channel_id: channelId,
    };

    if (options.filePath) {
      params.file = options.filePath;
      params.filename = options.filename || basename(options.filePath);
    } else if (options.content) {
      params.content = options.content;
      params.filename = options.filename;
    }

    if (options.title) params.title = options.title;
    if (options.initialComment) params.initial_comment = options.initialComment;
    if (options.snippetType) params.snippet_type = options.snippetType;
    if (options.threadTs) params.thread_ts = options.threadTs;

    const response = (await this.client.files.uploadV2(
      params as unknown as Parameters<typeof this.client.files.uploadV2>[0]
    )) as unknown as {
      ok?: boolean;
      files?: Array<{ files?: UploadedFileInfo[] }>;
    };

    const collected: UploadedFileInfo[] = [];
    for (const entry of response.files ?? []) {
      if (entry?.files) {
        collected.push(...entry.files);
      }
    }

    return { ok: response.ok, files: collected };
  }
}
