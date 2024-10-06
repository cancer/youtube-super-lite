import type { JSX, VoidComponent } from "solid-js";

type Props = {
  LeftSide: JSX.Element;
  RightSide: JSX.Element;
};
export const Header: VoidComponent<Props> = (props) => {
  return (
    <div class="grid">
      <div class="col-span-full flex justify-between">
        <div>{props.LeftSide}</div>
        <div>{props.RightSide}</div>
      </div>
    </div>
  );
};
