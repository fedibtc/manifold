import { createRequestLimit } from '../requestLimit';

interface DeferredTask {
  run: () => Promise<string>;
  settle: () => void;
  started: () => boolean;
}

const deferredTask = (id: string): DeferredTask => {
  let started = false;
  let resolve: (value: string) => void = () => {};
  const promise = new Promise<string>((resolveTask) => {
    resolve = resolveTask;
  });

  return {
    run: () => {
      started = true;
      return promise;
    },
    settle: () => resolve(id),
    started: () => started
  };
};

const flush = () => new Promise((resolve) => setTimeout(resolve, 0));

it('should run no more than the limit at once', async () => {
  const limit = createRequestLimit(2);
  const tasks = ['a', 'b', 'c'].map(deferredTask);

  tasks.forEach((task) => void limit(task.run));
  await flush();

  expect(tasks.map((task) => task.started())).toEqual([true, true, false]);
});

it('should start a waiting task as soon as a slot frees', async () => {
  const limit = createRequestLimit(2);
  const tasks = ['a', 'b', 'c'].map(deferredTask);
  tasks.forEach((task) => void limit(task.run));
  await flush();

  tasks[0].settle();
  await flush();

  expect(tasks[2].started()).toBe(true);
});

it('should run every queued task rather than dropping any', async () => {
  const limit = createRequestLimit(2);
  const done: string[] = [];
  const results = ['a', 'b', 'c', 'd', 'e'].map((id) =>
    limit(async () => {
      done.push(id);
      return id;
    })
  );

  await expect(Promise.all(results)).resolves.toEqual(['a', 'b', 'c', 'd', 'e']);
  expect(done).toHaveLength(5);
});

it('should free the slot when a task fails', async () => {
  const limit = createRequestLimit(1);

  await expect(limit(() => Promise.reject(new Error('boom')))).rejects.toThrow('boom');

  await expect(limit(() => Promise.resolve('next'))).resolves.toBe('next');
});
