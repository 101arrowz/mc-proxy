import { invoke } from '@tauri-apps/api';

export function begin() {
    return invoke<void>('begin');
}