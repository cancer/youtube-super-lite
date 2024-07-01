import "virtual:uno.css";
import "@unocss/reset/sanitize/sanitize.css";
import "./global.css";

// @refresh reload
import { Router } from "@solidjs/router";
import { FileRoutes } from "@solidjs/start/router";
import { ErrorBoundary, Suspense } from "solid-js";
import { BaseLayout } from "~/layouts/base";
import { BareLayout } from "./layouts/bare";

export default function App() {
  return (
    <ErrorBoundary
      fallback={(err) => {
        console.error(err);
        return (
          <BareLayout>
            <div>Error: {err.message}</div>
          </BareLayout>
        );
      }}
    >
      <Router
        root={(props) => (
          <BaseLayout>
            <Suspense>{props.children}</Suspense>
          </BaseLayout>
        )}
      >
        <FileRoutes />
      </Router>
    </ErrorBoundary>
  );
}
