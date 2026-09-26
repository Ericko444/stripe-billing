import { beforeEach, describe, expect, test, vi } from "vitest";
import { fireEvent, render, screen } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { LoginForm } from "./LoginForm";
import { apiRequest, ApiProblem } from "../../host/api/apiClient";

vi.mock("../../host/api/apiClient", async () => {
  const actual =
    await vi.importActual<typeof import("../../host/api/apiClient")>("../../host/api/apiClient");
  return { ...actual, apiRequest: vi.fn() };
});

const mockedApiRequest = vi.mocked(apiRequest);

function renderForm() {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  return render(
    <QueryClientProvider client={queryClient}>
      <LoginForm />
    </QueryClientProvider>,
  );
}

function fillAndSubmit() {
  fireEvent.change(screen.getByLabelText(/email/i), { target: { value: "alice@example.test" } });
  fireEvent.change(screen.getByLabelText(/password/i), {
    target: { value: "correct horse battery staple" },
  });
  fireEvent.click(screen.getByRole("button", { name: /log in/i }));
}

beforeEach(() => {
  mockedApiRequest.mockReset();
});

describe("LoginForm", () => {
  test("disables every input and the button while the attempt is pending", async () => {
    mockedApiRequest.mockReturnValue(new Promise(() => {}));
    renderForm();

    fillAndSubmit();

    expect(await screen.findByRole("button", { name: /logging in/i })).toBeDisabled();
    expect(screen.getByLabelText(/email/i)).toBeDisabled();
    expect(screen.getByLabelText(/password/i)).toBeDisabled();
  });

  test("shows the problem's detail on a refusal and clears the password", async () => {
    mockedApiRequest.mockRejectedValue(
      new ApiProblem(401, "Not authenticated", "The request could not be authenticated.", "c-1"),
    );
    renderForm();

    fillAndSubmit();

    expect(
      await screen.findByText("The request could not be authenticated."),
    ).toBeInTheDocument();
    expect(screen.getByLabelText(/password/i)).toHaveValue("");
    expect(screen.getByLabelText(/email/i)).toHaveValue("alice@example.test");
    expect(screen.getByRole("button", { name: /log in/i })).toBeEnabled();
  });
});
