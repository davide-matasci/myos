/* Minimal <netinet/in_systm.h> for myos (newlib lacks it; dropbear's
 * includes.h needs it before netinet/ip.h). */
#ifndef DROPBEAR_MYOS_IN_SYSTM_H
#define DROPBEAR_MYOS_IN_SYSTM_H

typedef unsigned short n_short; /* short int */
typedef unsigned int n_long;    /* long int */
typedef unsigned short n_time;  /* long int */

#endif /* DROPBEAR_MYOS_IN_SYSTM_H */
