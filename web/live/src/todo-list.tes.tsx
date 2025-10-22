import { render, fireEvent, cleanup } from "@solidjs/testing-library";

import { TodoList } from "./todo-list";
import { describe, expect, test, beforeEach } from "vitest";

describe("<TodoList />", () => {
  test("it will render an text input and a button", () => {
    const { getByPlaceholderText, getByText } = render(() => <TodoList />);
    expect(getByPlaceholderText("new todo here")).toBeInTheDocument();
    expect(getByText("Add Todo")).toBeInTheDocument();
  });

  test("it will add a new todo", async () => {
    const { getByPlaceholderText, getByText } = render(() => <TodoList />);
    const input = getByPlaceholderText("new todo here") as HTMLInputElement;
    const button = getByText("Add Todo");
    input.value = "test new todo";
    fireEvent.click(button as HTMLInputElement);
    expect(input.value).toBe("");
    expect(getByText(/test new todo/)).toBeInTheDocument();
  });

  test("it will mark a todo as completed", async () => {
    const { getByPlaceholderText, findByRole, getByText } = render(() => (
      <TodoList />
    ));
    const input = getByPlaceholderText("new todo here") as HTMLInputElement;
    const button = getByText("Add Todo") as HTMLButtonElement;
    input.value = "mark new todo as completed";
    fireEvent.click(button);
    const completed = (await findByRole("checkbox")) as HTMLInputElement;
    expect(completed?.checked).toBe(false);
    fireEvent.click(completed);
    expect(completed?.checked).toBe(true);
    const text = getByText("mark new todo as completed") as HTMLSpanElement;
    expect(text).toHaveStyle({ "text-decoration": "line-through" });
  });
});

describe.sequential("todolist-2", () => {
  let a = 1;

  test("it will render an text input and a button", () => {
    const { getByPlaceholderText, getByText } = render(() => <TodoList />);
    a = 2;
    expect(getByPlaceholderText("new todo here")).toBeInTheDocument();
    expect(getByText("Add Todo")).toBeInTheDocument();
    expect(true).toBe(false);
  });

  test("it will mark a todo as completed", async () => {
    const { getByPlaceholderText, findByRole, getByText } = render(() => (
      <TodoList />
    ));
    console.log(a);
    const input = getByPlaceholderText("new todo here") as HTMLInputElement;
    const button = getByText("Add Todo") as HTMLButtonElement;
    input.value = "mark new todo as completed";
    fireEvent.click(button);
    const completed = (await findByRole("checkbox")) as HTMLInputElement;
    expect(completed?.checked).toBe(false);
    fireEvent.click(completed);
    expect(completed?.checked).toBe(true);
    const text = getByText("mark new todo as completed") as HTMLSpanElement;
    expect(text).toHaveStyle({ "text-decoration": "line-through" });
  });

  beforeEach(() => {});
});

describe.sequential("seq-test", () => {
  let testValue = 0;
  test("test 1", () => {
    console.log("test 1", testValue);
    testValue = 1;
  });
  test("test 2", () => {
    console.log("test 2", testValue);
    expect(testValue).toBe(1);
  });
});
