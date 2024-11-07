import { type JSX, splitProps, type VoidComponent } from "solid-js";
import { parseYouTubeUrl, type YTNavigation } from "~/libs/url";

type Props = Omit<JSX.FormHTMLAttributes<HTMLFormElement>, "onSubmit"> & {
  onSubmit: (navigation: YTNavigation, triggerName: string) => void;
  Action: JSX.Element;
};
export const WatchVideoFromYouTube: VoidComponent<Props> = (props) => {
  const [localProps, restProps] = splitProps(props, ["onSubmit", "Action"]);
  return (
    <form
      {...restProps}
      onSubmit={(ev) => {
        ev.preventDefault();

        if (ev.currentTarget.url.value === "") return;

        const parsed = parseYouTubeUrl(ev.currentTarget.url.value);
        localProps.onSubmit(parsed, (ev.submitter as HTMLButtonElement).name);
        ev.currentTarget.url.value = "";
      }}
    >
      From YT URL: <input class="w-2xl h-10 text-xl" type="text" name="url" />
      {localProps.Action}
    </form>
  );
};
