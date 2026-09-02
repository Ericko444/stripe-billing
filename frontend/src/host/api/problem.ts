/**
 * Mirrors `crates/api/src/error.rs`'s `ProblemDetails` (RFC 9457). `detail`
 * is deliberately coarse there -- never the raw cause -- so every error
 * branch in this app renders `detail` as-is rather than trying to be
 * cleverer about it.
 */
export class ApiProblem extends Error {
  readonly status: number;
  readonly title: string;
  readonly detail: string;
  readonly correlationId: string;

  constructor(status: number, title: string, detail: string, correlationId: string) {
    super(detail);
    this.name = "ApiProblem";
    this.status = status;
    this.title = title;
    this.detail = detail;
    this.correlationId = correlationId;
  }
}

interface ProblemDetailsBody {
  type: string;
  title: string;
  status: number;
  detail: string;
  correlation_id: string;
}

function isProblemDetailsBody(value: unknown): value is ProblemDetailsBody {
  return (
    typeof value === "object" &&
    value !== null &&
    typeof (value as ProblemDetailsBody).title === "string" &&
    typeof (value as ProblemDetailsBody).status === "number" &&
    typeof (value as ProblemDetailsBody).detail === "string"
  );
}

/** Parses a non-ok `Response` into an `ApiProblem`, falling back to a
 * generic one if the body isn't the `application/problem+json` shape this
 * API always sends -- defensive against a proxy or the dev server itself
 * answering with something else (e.g. a raw 502 HTML page). */
export async function toApiProblem(response: Response): Promise<ApiProblem> {
  let body: unknown;
  try {
    body = await response.json();
  } catch {
    body = null;
  }

  if (isProblemDetailsBody(body)) {
    return new ApiProblem(body.status, body.title, body.detail, body.correlation_id);
  }

  return new ApiProblem(response.status, response.statusText || "Request failed", "", "");
}
