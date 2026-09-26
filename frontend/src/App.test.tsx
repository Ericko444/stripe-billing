import { beforeEach, describe, expect, test, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import App from "./App";
import { AuthProvider } from "./host/auth/AuthContext";
import { apiRequest, ApiProblem } from "./host/api/apiClient";

vi.mock("./host/api/apiClient", async () => {
  const actual = await vi.importActual<typeof import("./host/api/apiClient")>("./host/api/apiClient");
  return { ...actual, apiRequest: vi.fn() };
});

const mockedApiRequest = vi.mocked(apiRequest);

function renderShell() {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  return render(
    <QueryClientProvider client={queryClient}>
      <AuthProvider>
        <App />
      </AuthProvider>
    </QueryClientProvider>,
  );
}

beforeEach(() => {
  mockedApiRequest.mockReset();
});

describe("App shell", () => {
  test("shows the login form, and no 'session ended' notice, when there never was a session", async () => {
    mockedApiRequest.mockRejectedValue(
      new ApiProblem(401, "Not authenticated", "The request could not be authenticated.", "c-1"),
    );
    renderShell();

    expect(await screen.findByRole("heading", { name: /log in/i })).toBeInTheDocument();
    expect(screen.queryByText(/session ended/i)).not.toBeInTheDocument();
  });

  test("offers the tenant picker to a user with several memberships and no tenant yet", async () => {
    mockedApiRequest.mockResolvedValue({
      user_id: "u-1",
      email: "alice@example.test",
      display_name: "Alice",
      tenant: null,
      memberships: [
        { membership_id: "m-a", tenant_id: "t-a", tenant_name: "Tenant A", role: "owner" },
        { membership_id: "m-b", tenant_id: "t-b", tenant_name: "Tenant B", role: "member" },
      ],
      expires_at: "2026-09-13T20:00:00Z",
    });
    renderShell();

    expect(await screen.findByRole("heading", { name: /choose a tenant/i })).toBeInTheDocument();
  });

  test("says the server is unreachable when the session probe itself fails", async () => {
    mockedApiRequest.mockRejectedValue(new ApiProblem(502, "Bad Gateway", "", ""));
    renderShell();

    expect(await screen.findByText(/could not be reached/i)).toBeInTheDocument();
  });
});
