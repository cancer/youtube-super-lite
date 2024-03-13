import { type FlowComponent } from "solid-js";

export const BareLayout: FlowComponent = (props) => (
  <main class="bg-black h-full text-white">{props.children}</main>
);
