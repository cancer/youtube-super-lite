import { type FlowComponent } from "solid-js";

export const BaseLayout: FlowComponent = (props) => (
  <main class="bg-black h-full text-white">{props.children}</main>
);
