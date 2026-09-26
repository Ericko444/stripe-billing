import { beforeEach, describe, expect, test, vi } from "vitest";
import { fireEvent, render, screen } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { ForgotPasswordForm } from "./ForgotPasswordForm";
import { apiRequest } from "../../host/api/apiClient";

vi.mock("../../host/api/apiClient", async () => {
  const actual =
    await vi.importActual<typeof import("../../host/api/apiClient")>("../../host/api/apiClient");
  return { ...actual, apiRequest: vi.fn() };
});

const mockedApiRequest = vi.mocked(apiRequest);

function renderForm() {
  const queryClient = new QueryClient({ defaultOptions: { mutations: { retry: false } } });
  return render(
    <QueryClientProvider client={queryClient}>
      <ForgotPasswordForm onBack={vi.fn()} />
    </QueryClientProvider>,
  );
}

async function confirmationFor(address: string): Promise<string> {
  const view = renderForm();
  fireEvent.change(screen.getByLabelText(/email/i), { target: { value: address } });
  fireEvent.click(screen.getByRole("button", { name: /send the link/i }));
  const text = (await screen.findByRole("status")).textContent ?? "";
  view.unmount();
  return text;
}

beforeEach(() => {
  mockedApiRequest.mockReset();
  // The server's one fixed answer, whatever the address.
  mockedApiRequest.mockResolvedValue({
    message: "If an account exists for that address, a password reset link is on its way.",
  });
});

describe("ForgotPasswordForm", () => {
  test("shows the same confirmation for an address with and without an account", async () => {
    const known = await confirmationFor("alice@example.test");
    const unknown = await confirmationFor("nobody@example.test");

    expect(known).toBe(unknown);
    expect(known).toMatch(/if an account exists/i);
  });

  test("disables the form while the request is in flight", async () => {
    mockedApiRequest.mockReturnValue(new Promise(() => {}));
    renderForm();

    fireEvent.change(screen.getByLabelText(/email/i), { target: { value: "alice@example.test" } });
    fireEvent.click(screen.getByRole("button", { name: /send the link/i }));

    expect(await screen.findByRole("button", { name: /sending/i })).toBeDisabled();
    expect(screen.getByLabelText(/email/i)).toBeDisabled();
  });
});
