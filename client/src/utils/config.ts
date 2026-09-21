// Smart config: dynamically connects to port 3001 during dev (Vite on 5173),
// or directly to current origin when served in production or behind a reverse proxy.
export const SERVER_URL = window.location.port === '5173'
  ? `http://${window.location.hostname}:3001`
  : window.location.origin;

export const API_BASE = `${SERVER_URL}/api`;
