import { Action, ActionType } from '../actions';

export const DEFAULT: State = `fn main() {
    println!("Hello, world!");
}`;

export type State = string;

export default function code(state = DEFAULT, action: Action): State {
  switch (action.type) {
    case ActionType.RequestGistLoad:
      return '';
    case ActionType.GistLoadSucceeded:
      return action.code;

    case ActionType.EditCode:
      return action.code;

    case ActionType.AddMainFunction:
      return state + '\n\n' + DEFAULT;

    case ActionType.FormatSucceeded:
      return action.code;

    default:
      return state;
  }
}
