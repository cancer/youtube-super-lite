import { type VoidComponent } from "solid-js";
import IconParkSolidLike from "~icons/icon-park-solid/like";
import IconParkOutlineLike from "~icons/icon-park-outline/like";

type Props = {
  liked: boolean;
  onClick: () => void;
};
export const LikeButton: VoidComponent<Props> = (props) => (
  <>
    {props.liked ? (
      <span class="text-3xl">
        <IconParkSolidLike />
      </span>
    ) : (
      <button
        onClick={props.onClick}
        class="text-3xl text-white bg-transparent border-none appearance-none cursor-pointer"
      >
        <IconParkOutlineLike />
      </button>
    )}
  </>
);
