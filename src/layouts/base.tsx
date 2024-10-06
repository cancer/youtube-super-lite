import { type FlowComponent } from "solid-js";
import { BareLayout } from "~/layouts/bare";

export const BaseLayout: FlowComponent = (props) => {
  return <BareLayout>{props.children}</BareLayout>;
};
