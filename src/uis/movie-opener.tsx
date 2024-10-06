import type { VoidComponent } from "solid-js";

type Props = {
  openVideo: (videoId: string) => void;
};
export const MovieOpener: VoidComponent<Props> = (props) => (
  <form
    onSubmit={(ev) => {
      ev.preventDefault();
      const url = new URL(ev.currentTarget.url.value);
      props.openVideo(url.searchParams.get("v") ?? "");
      ev.currentTarget.url.value = "";
    }}
  >
    From YT URL: <input class="w-2xl h-10 text-xl" type="text" name="url" />
    <button type="submit">Watch</button>
  </form>
);
