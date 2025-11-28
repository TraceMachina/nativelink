import { For } from 'solid-js';
import { createStore } from 'solid-js/store';

type Todo = { id: number; text: string; completed: boolean };

export const TodoList = () => {
  let input!: HTMLInputElement;
  const [todos, setTodos] = createStore<Todo[]>([]);
  const addTodo = (text: string) => {
    setTodos(todos.length, { id: todos.length, text, completed: false });
  };
  const toggleTodo = (id: number) => {
    setTodos(id, 'completed', (c) => !c);
  };

  return (
    <>
      <div>
        <input placeholder="new todo here" ref={input} />
        <button
          onClick={() => {
            if (!input.value.trim()) return;
            addTodo(input.value);
            input.value = '';
          }}
        >
          Add Todo
        </button>
      </div>
      <div>
        <For each={todos}>
          {(todo) => {
            const { id, text } = todo;
            return (
              <div>
                <input
                  type="checkbox"
                  checked={todo.completed}
                  onchange={[toggleTodo, id]}
                />
                <span
                  style={{
                    'text-decoration': todo.completed ? 'line-through' : 'none',
                  }}
                >
                  {text}
                </span>
              </div>
            );
          }}
        </For>
      </div>
    </>
  );
};
