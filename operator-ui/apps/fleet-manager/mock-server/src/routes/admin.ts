import { type Request, type Response, Router, type Router as RouterType } from 'express';
import { adminMethods, dispatch } from '../../../src/mocks/world/verbs';

export const adminRouter: RouterType = Router();

export { adminMethods };

adminRouter.post('/', (req: Request, res: Response) => {
  res.json(dispatch(req.body));
});
