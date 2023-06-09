/*
 * Licensed to the Apache Software Foundation (ASF) under one
 * or more contributor license agreements.  See the NOTICE file
 * distributed with this work for additional information
 * regarding copyright ownership.  The ASF licenses this file
 * to you under the Apache License, Version 2.0 (the
 * "License"); you may not use this file except in compliance
 * with the License.  You may obtain a copy of the License at
 *
 *   http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing,
 * software distributed under the License is distributed on an
 * "AS IS" BASIS, WITHOUT WARRANTIES OR CONDITIONS OF ANY
 * KIND, either express or implied.  See the License for the
 * specific language governing permissions and limitations
 * under the License.
 *
 */

#include "teaclave_client_sdk.h"
#include <stdio.h>
#include <string.h>

#include "utils.h"

#define BUFFER_SIZE 4086
#define QUOTE(x...) #x

const char *register_function_request_serialized = QUOTE({
    "request" : "register_function",
    "name" : "builtin-echo",
    "description" : "Native Echo Function",
    "executor_type" : "builtin",
    "public" : true,
    "payload" : [],
    "arguments" : [{"key": "message", "default_value": "", "allow_overwrite": true}],
    "inputs" : [],
    "outputs" : [],
    "user_allowlist": [],
    "usage_quota": -1
});

const char *create_task_request_serialized = QUOTE({
    "request" : "create_task",
    "function_id" : "%s",
    "function_arguments" : "{\"message\": \"Hello, Teaclave!\"}",
    "executor" : "builtin",
    "inputs_ownership" : [],
    "outputs_ownership" : []
});

int main() {
    int ret = 0;

    char token[BUFFER_SIZE] = {0};
    size_t token_len = BUFFER_SIZE;
    char serialized_response[BUFFER_SIZE] = {0};
    char function_id[BUFFER_SIZE] = {0};
    char serialized_request[BUFFER_SIZE] = {0};
    char task_result[BUFFER_SIZE] = {0};
    char task_id[BUFFER_SIZE] = {0};
    const char *user_id = "test_id";
    const char *user_password = "test_password";
    const char *admin_user_id = "admin";
    const char *admin_user_password = "teaclave";

    /* Register */
    printf("[+] Registering\n");
    ret = login(admin_user_id, admin_user_password, token, &token_len);
    if (ret != 0) {
        fprintf(stderr, "[-] Failed to login.\n");
        goto bail;
    }

    ret = user_register(admin_user_id, token, user_id, user_password);
    if (ret != 0) {
        fprintf(stderr, "[-] Failed to register. Ignore.\n");
    }

    /* Login. */
    printf("[+] Login\n");
    token_len = BUFFER_SIZE;
    ret = login(user_id, user_password, token, &token_len);
    if (ret != 0) {
        fprintf(stderr, "[-] Failed to login.\n");
        goto bail;
    }

    /* Connect to the frontend serivice. */
    printf("connect frontend service\n");
    FrontendClient *frontend_client = teaclave_connect_frontend_service(
        frontend_service_address, enclave_info_path, as_root_ca_cert_path);
    if (frontend_client == NULL) {
        fprintf(stderr, "[-] Failed to connect to the frontend service.\n");
        ret = 1;
        goto bail;
    }

    printf("set crendential\n");
    /* Set user id and token. */
    ret = teaclave_frontend_set_credential(frontend_client, user_id, token);
    if (ret != 0) {
        fprintf(stderr, "[-] Failed to set credential.\n");
        goto bail;
    }

    /* Register function. */
    size_t serialized_response_len = BUFFER_SIZE;
    ret = teaclave_register_function_serialized(
        frontend_client, register_function_request_serialized,
        serialized_response, &serialized_response_len);
    if (ret != 0) {
        fprintf(stderr, "[-] Failed to register the function.\n");
        goto bail;
    }

    sscanf(serialized_response, "{\"function_id\":\"%45s", function_id);
    printf("[+] function_id: %s\n", function_id);

    /* Create task. */
    snprintf(serialized_request, BUFFER_SIZE, create_task_request_serialized,
             function_id);

    memset(serialized_response, 0, BUFFER_SIZE);
    ret = teaclave_create_task_serialized(frontend_client, serialized_request,
                                          serialized_response,
                                          &serialized_response_len);
    if (ret != 0) {
        fprintf(stderr, "[-] Failed to create a task.\n");
        goto bail;
    }

    sscanf(serialized_response, "{\"task_id\":\"%41s", task_id);
    printf("[+] task_id: %s\n", task_id);

    /* Invoke task. */
    ret = teaclave_invoke_task(frontend_client, task_id);
    if (ret != 0) {
        fprintf(stderr, "[-] Failed to invoke the task.\n");
        goto bail;
    }

    /* Get task result. */
    size_t task_result_len = BUFFER_SIZE;
    ret = teaclave_get_task_result(frontend_client, task_id, task_result,
                                   &task_result_len);
    if (ret != 0) {
        fprintf(stderr, "[-] Failed to get the task result.\n");
        goto bail;
    }

    printf("[+] Task result in string: %s\n", task_result);

    ret = teaclave_close_frontend_service(frontend_client);
    if (ret != 0) {
        fprintf(stderr, "[-] Failed to close the frontend service client.\n");
    }

    return ret;

bail:
    ret = teaclave_close_frontend_service(frontend_client);
    if (ret != 0) {
        fprintf(stderr, "[-] Failed to close the frontend service client.\n");
    }

    exit(-1);
}
