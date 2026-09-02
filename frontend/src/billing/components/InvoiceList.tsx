import { useState } from "react";
import { ApiProblem } from "../../host/api/apiClient";
import { useInvoices } from "../hooks/useInvoices";
import { formatMoney } from "../../money";

/** U3, 4b's keyset pagination -- pages by `next`, never an offset. "Load
 * more" replaces the visible page rather than accumulating; per P4 this is
 * the first thing cut under time pressure, so it stays deliberately simple. */
export function InvoiceList() {
  const [after, setAfter] = useState<string | undefined>(undefined);
  const query = useInvoices(after);

  if (query.isPending) {
    return <section aria-busy="true">Loading invoices…</section>;
  }

  if (query.isError) {
    const detail =
      query.error instanceof ApiProblem ? query.error.detail : "Something went wrong.";
    return <section role="alert">Could not load invoices: {detail}</section>;
  }

  const page = query.data;

  if (page.items.length === 0) {
    return (
      <section>
        <p>No invoices yet.</p>
      </section>
    );
  }

  return (
    <section>
      <ul>
        {page.items.map((invoice) => (
          <li key={invoice.id}>
            {formatMoney(invoice.amount)} — {invoice.status} — {invoice.created_at}
          </li>
        ))}
      </ul>
      {page.next !== undefined && (
        <button type="button" onClick={() => setAfter(page.next)}>
          Load more
        </button>
      )}
    </section>
  );
}
