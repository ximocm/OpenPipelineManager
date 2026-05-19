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
      if (typeof parsed.detail === 'string') message = parsed.detail;
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
