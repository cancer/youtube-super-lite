import type { JSX, VoidComponent } from "solid-js";
import "./index.css";

type Props = {
  LeftSide: JSX.Element;
  RightSide: JSX.Element;
};
export const Header: VoidComponent<Props> = (props) => {
  return (
    <div class="header">
      <div>{props.LeftSide}</div>
      <div>{props.RightSide}</div>
    </div>
  );
};
