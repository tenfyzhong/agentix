import { registerBridge } from '../runtime.mjs';
export default function bridge(api) { return registerBridge(api, 'omp'); }
