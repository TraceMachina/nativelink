/** @jsxImportSource react */
import { useState } from "react";

// import { qwikify$ } from "@builder.io/qwik-react";

export const ReactCounter = () => {
  const [count, setCount] = useState(0);

  return (
    <div className="flex justify-center items-center gap-5">
      <button type="button" onClick={() => setCount(count + 1)}>
        Update Counter
      </button>
      {count}
    </div>
  );
};

// export const QReactCounter = qwikify$(ReactCounter);
