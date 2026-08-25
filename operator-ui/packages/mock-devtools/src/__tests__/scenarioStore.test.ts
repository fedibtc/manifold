import { createScenarioStore, STORE_VERSION, storeKey } from '../scenario-store';
import type { StorageAdapter, WorldSource } from '../types';

interface TestWorld {
  seats: string[];
  price?: number | null;
}

const source: WorldSource<TestWorld> = {
  appKey: 'test',
  defaultScenario: 'empty',
  has: (name) => name === 'empty' || name === 'populated',
  build: (name) =>
    name === 'populated' ? { seats: ['seat-1'], price: 50 } : { seats: [], price: null }
};

const memoryStorage = (
  seed: Record<string, string> = {}
): StorageAdapter & {
  contents: Record<string, string>;
} => {
  const contents = { ...seed };
  return {
    contents,
    load: (key) => contents[key] ?? null,
    save: (key, value) => {
      contents[key] = value;
    }
  };
};

const KEY = 'operator-ui:dev:mocks:test';

it('should build the default scenario when nothing is persisted', () => {
  const store = createScenarioStore(source, memoryStorage());

  expect(store.getScenario()).toBe('empty');
  expect(store.getWorld().seats).toEqual([]);
});

it('should rehydrate a persisted world including its mutations', () => {
  const storage = memoryStorage({
    [KEY]: JSON.stringify({
      v: STORE_VERSION,
      scenario: 'populated',
      world: { seats: ['seat-1', 'seat-mutated'], price: 99 }
    })
  });

  const store = createScenarioStore(source, storage);

  expect(store.getScenario()).toBe('populated');
  expect(store.getWorld().seats).toEqual(['seat-1', 'seat-mutated']);
  expect(store.getWorld().price).toBe(99);
});

it('should write the mutated world back when persist is called', () => {
  const storage = memoryStorage();
  const store = createScenarioStore(source, storage);

  store.getWorld().seats.push('seat-added');
  store.persist();

  expect(JSON.parse(storage.contents[KEY]).world.seats).toEqual(['seat-added']);
});

it('should discard a world persisted under a different store version', () => {
  const storage = memoryStorage({
    [KEY]: JSON.stringify({
      v: STORE_VERSION + 1,
      scenario: 'populated',
      world: { seats: ['stale'], price: 1 }
    })
  });

  const store = createScenarioStore(source, storage);

  expect(store.getScenario()).toBe('empty');
  expect(store.getWorld().seats).toEqual([]);
});

it('should discard a world whose scenario name is no longer known', () => {
  const storage = memoryStorage({
    [KEY]: JSON.stringify({
      v: STORE_VERSION,
      scenario: 'deleted-scenario',
      world: { seats: ['stale'], price: 1 }
    })
  });

  const store = createScenarioStore(source, storage);

  expect(store.getScenario()).toBe('empty');
});

it('should fall back to the default when the persisted blob is not valid JSON', () => {
  const store = createScenarioStore(source, memoryStorage({ [KEY]: '{not json' }));

  expect(store.getScenario()).toBe('empty');
});

it('should rebuild from the named scenario and drop earlier mutations on switch', () => {
  const storage = memoryStorage();
  const store = createScenarioStore(source, storage);
  store.getWorld().seats.push('seat-added');
  store.persist();

  store.setScenario('populated');

  expect(store.getWorld().seats).toEqual(['seat-1']);
  expect(JSON.parse(storage.contents[KEY]).scenario).toBe('populated');
});

it('should return to the default scenario on reset', () => {
  const store = createScenarioStore(source, memoryStorage());
  store.setScenario('populated');

  store.reset();

  expect(store.getScenario()).toBe('empty');
  expect(store.getWorld().seats).toEqual([]);
});

it('should build a fresh world from a seed marker written before boot', () => {
  const store = createScenarioStore(
    source,
    memoryStorage({ [KEY]: JSON.stringify({ seed: 'populated' }) })
  );

  expect(store.getScenario()).toBe('populated');
  expect(store.getWorld().seats).toEqual(['seat-1']);
});

it('should notify subscribers when the scenario changes', () => {
  const store = createScenarioStore(source, memoryStorage());
  let calls = 0;
  const unsubscribe = store.subscribe(() => {
    calls += 1;
  });

  store.setScenario('populated');
  unsubscribe();
  store.setScenario('empty');

  expect(calls).toBe(1);
});

it('should report the scenario that reset returns to', () => {
  const store = createScenarioStore(source, memoryStorage());
  store.setScenario('populated');

  expect(store.getDefaultScenario()).toBe('empty');
});

it('should build the storage key from the app key', () => {
  expect(storeKey('flip')).toBe('operator-ui:dev:mocks:flip');
});

it('should carry state across a scenario switch when the source asks for it', () => {
  const carrying: WorldSource<TestWorld & { session: boolean }> = {
    appKey: 'test',
    defaultScenario: 'empty',
    has: (name) => name === 'empty' || name === 'populated',
    build: (name) => ({ seats: name === 'populated' ? ['seat-1'] : [], session: false }),
    carryOver: (previous, next) => {
      next.session = previous.session;
    }
  };
  const store = createScenarioStore(carrying, memoryStorage());
  store.getWorld().session = true;

  store.setScenario('populated');

  expect(store.getWorld().session).toBe(true);
  expect(store.getWorld().seats).toEqual(['seat-1']);
});

it('should let setScenario surface a broken builder rather than swallow it', () => {
  const exploding: WorldSource<TestWorld> = {
    appKey: 'test',
    defaultScenario: 'empty',
    has: () => true,
    build: (name) => {
      if (name === 'broken') throw new Error('builder is wrong');
      return { seats: [] };
    }
  };
  const store = createScenarioStore(exploding, memoryStorage());

  expect(() => store.setScenario('broken')).toThrow('builder is wrong');
});

it('should not swallow a scenario builder that throws while restoring a seed', () => {
  const exploding: WorldSource<TestWorld> = {
    appKey: 'test',
    defaultScenario: 'empty',
    has: () => true,
    build: (name) => {
      if (name === 'broken') throw new Error('builder is wrong');
      return { seats: [] };
    }
  };
  const storage = memoryStorage({ [storeKey('test')]: JSON.stringify({ seed: 'broken' }) });

  expect(() => createScenarioStore(exploding, storage)).toThrow('builder is wrong');
});

it('should not notify subscribers when a verb persists the world', () => {
  const store = createScenarioStore(source, memoryStorage());
  let calls = 0;
  store.subscribe(() => {
    calls += 1;
  });

  store.persist();

  expect(calls).toBe(0);
});

it('should notify subscribers when a control announces a change', () => {
  const store = createScenarioStore(source, memoryStorage());
  let calls = 0;
  store.subscribe(() => {
    calls += 1;
  });

  store.notify();

  expect(calls).toBe(1);
});

it('should leave the revision alone when a verb persists the world', () => {
  const store = createScenarioStore(source, memoryStorage());
  const before = store.getRevision();

  store.persist();

  expect(store.getRevision()).toBe(before);
});

it('should advance the revision on every notification', () => {
  const store = createScenarioStore(source, memoryStorage());
  const before = store.getRevision();

  store.notify();
  store.notify();

  expect(store.getRevision()).toBe(before + 2);
});

it('should advance the revision when the scenario changes', () => {
  const store = createScenarioStore(source, memoryStorage());
  const before = store.getRevision();

  store.setScenario('populated');

  expect(store.getRevision()).toBe(before + 1);
});

it('should start clean', () => {
  const store = createScenarioStore(source, memoryStorage());

  expect(store.isDirty()).toBe(false);
});

it('should mark the world dirty when a control announces a change', () => {
  const store = createScenarioStore(source, memoryStorage());

  store.notify();

  expect(store.isDirty()).toBe(true);
});

it('should not mark the world dirty when a verb only persists', () => {
  const store = createScenarioStore(source, memoryStorage());

  store.getWorld().seats.push('seat-added');
  store.persist();

  expect(store.isDirty()).toBe(false);
});

it('should persist the dirty flag so a reload still shows the override', () => {
  const storage = memoryStorage();
  const store = createScenarioStore(source, storage);
  store.notify();

  const reloaded = createScenarioStore(source, storage);

  expect(JSON.parse(storage.contents[KEY]).dirty).toBe(true);
  expect(reloaded.isDirty()).toBe(true);
});

it('should come back clean when the scenario changes', () => {
  const store = createScenarioStore(source, memoryStorage());
  store.notify();

  store.setScenario('populated');

  expect(store.isDirty()).toBe(false);
});

it('should come back clean on reset', () => {
  const store = createScenarioStore(source, memoryStorage());
  store.notify();

  store.reset();

  expect(store.isDirty()).toBe(false);
});

it('should start clean from a seed marker', () => {
  const store = createScenarioStore(
    source,
    memoryStorage({ [KEY]: JSON.stringify({ seed: 'populated' }) })
  );

  expect(store.isDirty()).toBe(false);
});

it('should export the app key, scenario and world as the debug recipe', () => {
  const store = createScenarioStore(source, memoryStorage());
  store.setScenario('populated');
  store.notify();

  const recipe = JSON.parse(store.exportState());

  expect(recipe).toEqual({
    app: 'test',
    v: STORE_VERSION,
    scenario: 'populated',
    world: { seats: ['seat-1'], price: 50 },
    dirty: true
  });
});
