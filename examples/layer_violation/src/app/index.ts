// OK: app may import from both domain and infra.
import { newUser } from '../domain/user';
import { connect } from '../infra/connection';

connect();
newUser('1');
