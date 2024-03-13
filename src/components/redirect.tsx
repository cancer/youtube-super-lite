import { HttpHeader, HttpStatusCode } from "@solidjs/start";
import { type VoidComponent } from "solid-js";

type Props = { path: string };
export const Redirect: VoidComponent<Props> = (props) => (
  <>
    <HttpStatusCode code={302} />
    <HttpHeader name="Location" value={props.path} />
  </>
);
