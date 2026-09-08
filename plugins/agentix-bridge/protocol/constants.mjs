import schema from './schema.json' with { type: 'json' };
export const PROTOCOL_VERSION = schema['x-agentix'].version;
export const MAX_FRAME_BYTES = schema['x-agentix'].maxFrameBytes;
