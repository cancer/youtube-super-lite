import { useNavigate } from "@solidjs/router";
import { type JSX, type VoidComponent } from "solid-js";

type Props = {
  Login: JSX.Element;
};
export const Header: VoidComponent<Props> = (props) => {
  const navigate = useNavigate();

  return (
    <div class="grid">
      <div class="col-span-full flex justify-between">
        <form
          onSubmit={(ev) => {
            ev.preventDefault();
            const url = new URL(ev.currentTarget.url.value);
            navigate(`/watch/${url.searchParams.get("v") ?? ""}`);
            ev.currentTarget.url.value = "";
          }}
        >
          From YT URL:{" "}
          <input class="w-2xl h-10 text-xl" type="text" name="url" />
          <button type="submit">Watch</button>
        </form>
        {props.Login}
      </div>
    </div>
  );
};
