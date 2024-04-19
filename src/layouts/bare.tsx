import { type FlowComponent } from "solid-js";

export const BareLayout: FlowComponent = (props) => (
  <main class="bg-black text-white min-h-dvh">{props.children}</main>
);
