import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";
import { fireEvent, render, screen } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { ResetPasswordForm } from "./ResetPasswordForm";
import { apiRequest, ApiProblem } from "../../host/api/apiClient";

vi.mock("../../host/api/apiClient", async () => {
  const actual =
    await vi.importActual<typeof import("../../host/api/apiClient")>("../../host/api/apiClient");
  return { ...actual, apiRequest: vi.fn() };
});

const mockedApiRequest = vi.mocked(apiRequest);
const NEW_PASSWORD = "a much longer and newer passphrase";

function renderForm() {
  const queryClient = new QueryClient({ defaultOptions: { mutations: { retry: false } } });
  return render(
    <QueryClientProvider client={queryClient}>
      <ResetPasswordForm onRequestNewLink={vi.fn()} onDone={vi.fn()} />
    </QueryClientProvider>,
  );
}

function setNewPassword(password: string, confirmation = password) {
  fireEvent.change(screen.getByLabelText(/new password/i), { target: { value: password } });
  fireEvent.change(screen.getByLabelText(/repeat it/i), { target: { value: confirmation } });
  fireEvent.click(screen.getByRole("button", { name: /set the new password/i }));
}

beforeEach(() => {
  mockedApiRequest.mockReset();
});

afterEach(() => {
  window.history.replaceState(null, "", "/");
});

describe("ResetPasswordForm", () => {
  test("sends the token from the fragment, never the query, and strips it from the address bar", async () => {
    // A query-string token is planted to prove it is ignored.
    window.history.replaceState(
      null,
      "",
      "/reset-password?token=from-the-query#token=from-the-fragment",
    );
    const replaceState = vi.spyOn(window.history, "replaceState");
    mockedApiRequest.mockResolvedValue(undefined);

    renderForm();

    expect(replaceState).toHaveBeenCalledWith(null, "", "/reset-password");
    expect(window.location.hash).toBe("");

    setNewPassword(NEW_PASSWORD);
    expect(await screen.findByRole("status")).toHaveTextContent(/log in with your new password/i);

    const [path, init] = mockedApiRequest.mock.calls[0] ?? [];
    expect(path).toBe("/auth/password-reset/complete");
    const body = JSON.parse(String(init?.body));
    expect(body).toEqual({ token: "from-the-fragment", new_password: NEW_PASSWORD });
    expect(JSON.stringify(mockedApiRequest.mock.calls)).not.toContain("from-the-query");
    replaceState.mockRestore();
  });

  test("shows the single invalid-link state when the server refuses the link", async () => {
    window.history.replaceState(null, "", "/reset-password#token=aa.bb");
    mockedApiRequest.mockRejectedValue(
      new ApiProblem(400, "Invalid link", "This link is invalid or has expired.", "c-1"),
    );
    renderForm();

    setNewPassword(NEW_PASSWORD);

    expect(await screen.findByRole("heading", { name: /can't be used/i })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /request a new link/i })).toBeInTheDocument();
  });

  test("shows the invalid-link state straight away when there is no token at all", () => {
    window.history.replaceState(null, "", "/reset-password");
    renderForm();

    expect(screen.getByRole("heading", { name: /can't be used/i })).toBeInTheDocument();
    expect(mockedApiRequest).not.toHaveBeenCalled();
  });

  test("checks length and confirmation before any request", () => {
    window.history.replaceState(null, "", "/reset-password#token=aa.bb");
    renderForm();

    setNewPassword("short");
    expect(screen.getByRole("alert")).toHaveTextContent(/at least 15 characters/i);

    setNewPassword(NEW_PASSWORD, `${NEW_PASSWORD}!`);
    expect(screen.getByRole("alert")).toHaveTextContent(/do not match/i);

    expect(mockedApiRequest).not.toHaveBeenCalled();
  });
});
