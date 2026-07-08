import { createWriteStream } from 'fs';
import { basename, join } from 'path';
import { pipeline } from 'stream/promises';
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

export interface DownloadFileOptions {
  url?: string;
  fileId?: string;
  outputDir?: string;
  outputPath?: string;
}

export interface DownloadFileResult {
  filePath: string;
  fileName: string;
  size: number;
}

export class FileOperations extends BaseSlackClient {
  private channelOps: ChannelOperations;

  constructor(dependency: SlackClientDependency, channelOps?: ChannelOperations) {
    super(dependency);
    this.channelOps = channelOps ?? new ChannelOperations(dependency);
  }

  async downloadFile(options: DownloadFileOptions): Promise<DownloadFileResult> {
    let url: string;
    let fileName: string;

    if (options.fileId) {
      const info = (await this.client.files.info({ file: options.fileId })) as {
        file?: { url_private_download?: string; url_private?: string; name?: string };
      };
      url = info.file?.url_private_download || info.file?.url_private || '';
      fileName = info.file?.name || options.fileId;
      if (!url) throw new Error('No download URL found for this file');
    } else if (options.url) {
      url = options.url;
      const urlPath = new URL(url).pathname;
      fileName = decodeURIComponent(basename(urlPath));
    } else {
      throw new Error('Either --url or --id is required');
    }

    const outputPath = options.outputPath || join(options.outputDir || '.', fileName);
    const token = this.client.token;
    if (!token) throw new Error('No token available');

    const response = await fetch(url, {
      headers: { Authorization: `Bearer ${token}` },
    });

    if (!response.ok) {
      throw new Error(`Download failed: ${response.status} ${response.statusText}`);
    }
    if (!response.body) {
      throw new Error('No response body');
    }

    const fileStream = createWriteStream(outputPath);
    await pipeline(response.body as unknown as NodeJS.ReadableStream, fileStream);

    const { size } = await import('fs').then((fs) => fs.promises.stat(outputPath));

    return { filePath: outputPath, fileName, size };
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
      error?: string;
      files?: Array<{ ok?: boolean; error?: string; files?: UploadedFileInfo[] }>;
    };

    if (response.ok === false) {
      throw new Error(response.error ?? 'files.uploadV2 failed');
    }

    const collected: UploadedFileInfo[] = [];
    for (const entry of response.files ?? []) {
      if (entry.ok === false) {
        throw new Error(entry.error ?? 'completeUploadExternal failed');
      }
      if (entry.files) {
        collected.push(...entry.files);
      }
    }

    return { ok: response.ok, files: collected };
  }
}
