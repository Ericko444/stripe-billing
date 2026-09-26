import { beforeEach, describe, expect, test, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { SubscriptionPanel } from "./SubscriptionPanel";
import { apiRequest, ApiProblem } from "../../host/api/apiClient";

vi.mock("../../host/api/apiClient", async () => {
  const actual =
    await vi.importActual<typeof import("../../host/api/apiClient")>(
      "../../host/api/apiClient",
    );
  return { ...actual, apiRequest: vi.fn() };
});

vi.mock("../../host/auth/AuthContext", () => ({
  useAuth: () => ({
    tenantId: "00000000-0000-0000-0000-000000000001",
    switchTenant: vi.fn(),
  }),
}));

const mockedApiRequest = vi.mocked(apiRequest);

function renderPanel() {
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={queryClient}>
      <SubscriptionPanel />
    </QueryClientProvider>,
  );
}

beforeEach(() => {
  mockedApiRequest.mockReset();
});

describe("SubscriptionPanel", () => {
  test("shows a loading state while the query is pending", () => {
    mockedApiRequest.mockReturnValue(new Promise(() => {}));
    renderPanel();
    expect(screen.getByText(/loading your subscription/i)).toBeInTheDocument();
  });

  test("shows the problem's detail on error", async () => {
    mockedApiRequest.mockRejectedValue(
      new ApiProblem(500, "Internal error", "the server exploded", "corr-1"),
    );
    renderPanel();
    expect(await screen.findByText(/the server exploded/i)).toBeInTheDocument();
  });

  test("shows the no-subscription call to action for a null subscription", async () => {
    // The no-subscription branch also renders a plan choice (`/plans`), so
    // the mock must distinguish endpoints rather than resolve every call
    // the same way.
    mockedApiRequest.mockImplementation((path: string) =>
      Promise.resolve(path === "/plans" ? [] : null),
    );
    renderPanel();
    expect(await screen.findByText(/don't have a subscription yet/i)).toBeInTheDocument();
  });
});
