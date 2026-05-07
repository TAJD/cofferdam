// OK: app → domain and app → infra are allowed.
import { newUser } from '../domain/user';
import { connect } from '../infra/connection';

connect();
newUser('1');
