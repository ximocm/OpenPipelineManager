const API_BASE = import.meta.env.VITE_API_BASE_URL ?? 'http://localhost:8000';

export async function apiRequest<T>(path: string, options: RequestInit = {}): Promise<T> {
  const headers = new Headers(options.headers);
  headers.set('Content-Type', 'application/json');

  const response = await fetch(`${API_BASE}${path}`, {
    ...options,
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
