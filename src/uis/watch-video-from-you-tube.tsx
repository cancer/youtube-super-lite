import { JSX, splitProps, VoidComponent } from "solid-js";

type Props = JSX.FormHTMLAttributes<HTMLFormElement> & {
  Action: JSX.Element;
};
export const WatchVideoFromYouTube: VoidComponent<Props> = (props) => {
  const [localProps, restProps] = splitProps(props, ["Action"]);
  return (
    <form {...restProps}>
      From YT URL: <input class="w-2xl h-10 text-xl" type="text" name="url" />
      {localProps.Action}
    </form>
  );
};
