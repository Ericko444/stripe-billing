import { beforeEach, expect, test, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { InvoiceList } from "./InvoiceList";
import { apiRequest } from "../../host/api/apiClient";

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

beforeEach(() => {
  mockedApiRequest.mockReset();
});

test("InvoiceList renders its empty state for an empty page", async () => {
  // The real wire omits `next` entirely on an empty/last page
  // (`crates/api/src/dto.rs`'s `skip_serializing_if`), not `next: null`.
  mockedApiRequest.mockResolvedValue({ items: [] });

  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={queryClient}>
      <InvoiceList />
    </QueryClientProvider>,
  );

  expect(await screen.findByText(/no invoices yet/i)).toBeInTheDocument();
});
