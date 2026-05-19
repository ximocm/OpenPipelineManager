const API_BASE = import.meta.env.VITE_API_BASE_URL ?? 'http://localhost:8000';

let csrfToken: string | undefined;
let csrfTokenRequest: Promise<string> | undefined;

async function getCsrfToken(): Promise<string> {
  if (csrfToken) return csrfToken;
  if (!csrfTokenRequest) {
    csrfTokenRequest = fetch(`${API_BASE}/api/security/csrf-token`)
      .then(async (response) => {
        if (!response.ok) throw new Error('Unable to obtain CSRF token');
        const payload = (await response.json()) as { token: string };
        csrfToken = payload.token;
        return payload.token;
      })
      .finally(() => {
        csrfTokenRequest = undefined;
      });
  }
  return csrfTokenRequest;
}

function requiresCsrf(method: string): boolean {
  return !['GET', 'HEAD', 'OPTIONS'].includes(method.toUpperCase());
}

export async function apiRequest<T>(path: string, options: RequestInit = {}): Promise<T> {
  const headers = new Headers(options.headers);
  headers.set('Content-Type', 'application/json');
  const method = options.method ?? 'GET';
  if (requiresCsrf(method)) {
    headers.set('X-OPM-CSRF-Token', await getCsrfToken());
  }

  const response = await fetch(`${API_BASE}${path}`, {
    ...options,
    method,
    headers,
  });
  if (!response.ok) {
    const detail = await response.text();
    let message = detail || response.statusText;
    try {
      const parsed = JSON.parse(detail) as { detail?: unknown };
      if (typeof parsed.detail === 'string') {
        message = parsed.detail;
      } else if (isRecord(parsed.detail)) {
        message = formatErrorDetail(parsed.detail);
      }
    } catch {
      // Keep the raw response text when the backend did not return JSON.
    }
    throw new Error(message);
  }
  return response.json() as Promise<T>;
}

export function apiUrl(path: string): string {
  return `${API_BASE}${path}`;
}

function formatErrorDetail(detail: Record<string, unknown>): string {
  const message = typeof detail.message === 'string' ? detail.message : 'Request failed';
  const blockers = Array.isArray(detail.blockers) ? detail.blockers.map(formatBlocker).filter(Boolean) : [];
  return blockers.length ? `${message} ${blockers.join('; ')}` : message;
}

function formatBlocker(value: unknown): string {
  if (!isRecord(value)) return '';
  const stepId = typeof value.step_id === 'string' ? value.step_id : 'pipeline';
  const field = typeof value.field === 'string' ? value.field : '';
  const message = typeof value.message === 'string' ? value.message : '';
  const location = field ? `${stepId}.${field}` : stepId;
  return message ? `${location}: ${message}` : location;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}
