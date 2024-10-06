import { useNavigate } from "@solidjs/router";
import type { VoidComponent } from "solid-js";

export const MovieOpener: VoidComponent = () => {
  const navigate = useNavigate();
  return (
    <form
      onSubmit={(ev) => {
        ev.preventDefault();
        const url = new URL(ev.currentTarget.url.value);
        navigate(`/watch/${url.searchParams.get("v") ?? ""}`);
        ev.currentTarget.url.value = "";
      }}
    >
      From YT URL: <input class="w-2xl h-10 text-xl" type="text" name="url" />
      <button type="submit">Watch</button>
    </form>
  );
};
