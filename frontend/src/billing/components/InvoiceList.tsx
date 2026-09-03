import { useState } from "react";
import { ApiProblem } from "../../host/api/apiClient";
import { useInvoices } from "../hooks/useInvoices";
import { formatMoney } from "../../money";
import { formatDate } from "../../format";
import { Alert, Badge, Button, Card, EmptyState, Loading } from "../../ui/primitives";
import type { InvoiceStatus } from "../api/types";

const STATUS_TONE: Record<InvoiceStatus, "positive" | "caution" | "negative"> = {
  paid: "positive",
  open: "caution",
  failed: "negative",
};

/** U3, 4b's keyset pagination -- pages by `next`, never an offset. "Load
 * more" replaces the visible page rather than accumulating; per P4 this is
 * the first thing cut under time pressure, so it stays deliberately simple. */
export function InvoiceList() {
  const [after, setAfter] = useState<string | undefined>(undefined);
  const query = useInvoices(after);

  if (query.isPending) {
    return (
      <Card title="Invoices">
        <Loading>Loading invoices…</Loading>
      </Card>
    );
  }

  if (query.isError) {
    const detail =
      query.error instanceof ApiProblem ? query.error.detail : "Something went wrong.";
    return (
      <Card title="Invoices">
        <Alert>Could not load invoices: {detail}</Alert>
      </Card>
    );
  }

  const page = query.data;

  if (page.items.length === 0) {
    return (
      <Card title="Invoices">
        <EmptyState>No invoices yet.</EmptyState>
      </Card>
    );
  }

  return (
    <Card title="Invoices">
      <div className="stack">
        <ul style={{ listStyle: "none", margin: 0, padding: 0 }}>
          {page.items.map((invoice) => (
            <li key={invoice.id} className="row">
              <span className="mono">{formatMoney(invoice.amount)}</span>
              <span className="muted">{formatDate(invoice.created_at)}</span>
              <Badge tone={STATUS_TONE[invoice.status]}>{invoice.status}</Badge>
            </li>
          ))}
        </ul>
        {page.next !== undefined && (
          <Button
            variant="secondary"
            style={{ alignSelf: "flex-start" }}
            onClick={() => setAfter(page.next)}
          >
            Load more
          </Button>
        )}
      </div>
    </Card>
  );
}
