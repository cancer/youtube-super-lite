import { clientOnly } from "@solidjs/start";
import { createSignal } from "solid-js";

const Player = clientOnly(() =>
  import("~/components/player").then(({ Player }) => ({ default: Player })),
);

export default function Index() {
  const [videoId, setVideoId] = createSignal("");
  return (
    <main class="bg-black h-full">
      <form
        onSubmit={(ev) => {
          ev.preventDefault();
          const url = new URL((ev.currentTarget as HTMLFormElement).url.value);
          setVideoId(url.searchParams.get("v") ?? "");
        }}
      >
        <input class="w-2xl h-10 text-xl" type="text" name="url" />
        <button type="submit">Watch</button>
      </form>
      <Player videoId={videoId()} />
    </main>
  );
}
