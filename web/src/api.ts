export async function api(path: string, options: RequestInit = {}) {
  const response = await fetch("/api" + path, {
    ...options,
    headers: {
      "Content-Type": "application/json",
      "X-Web-Request": "1",
      "X-Web-Version": __WEB_VERSION__,
      ...options.headers,
    },
  });
  const body = await response
    .json()
    .catch(() => ({ error: response.statusText }));
  if (!response.ok) {
    const error = new Error(body.error || JSON.stringify(body));
    Object.assign(error, { status: response.status, body });
    throw error;
  }
  return body;
}
export const send = (path: string, body: unknown, method = "POST") =>
  api(path, { method, body: JSON.stringify(body) });
export const pretty = (value: unknown) => JSON.stringify(value, null, 2);
export const errorMessage = (error: unknown) => (error as Error).message;
