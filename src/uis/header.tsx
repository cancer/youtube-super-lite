import type { JSX, VoidComponent } from "solid-js";

type Props = {
  LeftSide: JSX.Element;
  RightSide: JSX.Element;
};
export const Header: VoidComponent<Props> = (props) => {
  return (
    <div class="flex justify-between items-center">
      <div>{props.LeftSide}</div>
      <div>{props.RightSide}</div>
    </div>
  );
};
