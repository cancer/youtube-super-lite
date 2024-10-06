import type { JSX, VoidComponent } from "solid-js";

type Props = {
  MovieOpener: JSX.Element;
  Login: JSX.Element;
};
export const Header: VoidComponent<Props> = (props) => {
  return (
    <div class="grid">
      <div class="col-span-full flex justify-between">
        {props.MovieOpener}
        {props.Login}
      </div>
    </div>
  );
};
