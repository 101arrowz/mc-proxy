import { invoke, InvokeArgs } from '@tauri-apps/api/tauri';

interface BeginProxyOptions {
    username?: string;
    password?: string;
    apiKey?: string;
}

export function begin(opts?: BeginProxyOptions) {
    return invoke<void>('begin', opts as InvokeArgs);
}

export function msFlow(apiKey?: string) {
    return invoke<void>('ms_flow', { apiKey });
}