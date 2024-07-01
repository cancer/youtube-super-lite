import { type FlowComponent } from "solid-js";
import { Header } from "~/components/header";
import { BareLayout } from "~/layouts/bare";

export const BaseLayout: FlowComponent = (props) => {
  return (
    <BareLayout>
      <Header />
      {props.children}
    </BareLayout>
  );
};
