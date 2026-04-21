# JobExecutionResponse

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**CompletedAt** | Pointer to **NullableString** | When the job completed (ISO 8601 format) | [optional] 
**DurationMs** | Pointer to **NullableInt64** | Duration in milliseconds (if completed) | [optional] 
**Error** | Pointer to **NullableString** | Error reason (if failed) | [optional] 
**ExitCode** | Pointer to **NullableInt32** | Exit code (if completed/failed) | [optional] 
**Id** | **string** | Unique execution ID | 
**JobName** | **string** | Name of the job | 
**Logs** | Pointer to **NullableString** | Captured logs | [optional] 
**StartedAt** | Pointer to **NullableString** | When the job started (ISO 8601 format) | [optional] 
**Status** | **string** | Current status (pending, initializing, running, completed, failed, cancelled) | 
**Trigger** | **string** | How the job was triggered | 

## Methods

### NewJobExecutionResponse

`func NewJobExecutionResponse(id string, jobName string, status string, trigger string, ) *JobExecutionResponse`

NewJobExecutionResponse instantiates a new JobExecutionResponse object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewJobExecutionResponseWithDefaults

`func NewJobExecutionResponseWithDefaults() *JobExecutionResponse`

NewJobExecutionResponseWithDefaults instantiates a new JobExecutionResponse object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetCompletedAt

`func (o *JobExecutionResponse) GetCompletedAt() string`

GetCompletedAt returns the CompletedAt field if non-nil, zero value otherwise.

### GetCompletedAtOk

`func (o *JobExecutionResponse) GetCompletedAtOk() (*string, bool)`

GetCompletedAtOk returns a tuple with the CompletedAt field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetCompletedAt

`func (o *JobExecutionResponse) SetCompletedAt(v string)`

SetCompletedAt sets CompletedAt field to given value.

### HasCompletedAt

`func (o *JobExecutionResponse) HasCompletedAt() bool`

HasCompletedAt returns a boolean if a field has been set.

### SetCompletedAtNil

`func (o *JobExecutionResponse) SetCompletedAtNil(b bool)`

 SetCompletedAtNil sets the value for CompletedAt to be an explicit nil

### UnsetCompletedAt
`func (o *JobExecutionResponse) UnsetCompletedAt()`

UnsetCompletedAt ensures that no value is present for CompletedAt, not even an explicit nil
### GetDurationMs

`func (o *JobExecutionResponse) GetDurationMs() int64`

GetDurationMs returns the DurationMs field if non-nil, zero value otherwise.

### GetDurationMsOk

`func (o *JobExecutionResponse) GetDurationMsOk() (*int64, bool)`

GetDurationMsOk returns a tuple with the DurationMs field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetDurationMs

`func (o *JobExecutionResponse) SetDurationMs(v int64)`

SetDurationMs sets DurationMs field to given value.

### HasDurationMs

`func (o *JobExecutionResponse) HasDurationMs() bool`

HasDurationMs returns a boolean if a field has been set.

### SetDurationMsNil

`func (o *JobExecutionResponse) SetDurationMsNil(b bool)`

 SetDurationMsNil sets the value for DurationMs to be an explicit nil

### UnsetDurationMs
`func (o *JobExecutionResponse) UnsetDurationMs()`

UnsetDurationMs ensures that no value is present for DurationMs, not even an explicit nil
### GetError

`func (o *JobExecutionResponse) GetError() string`

GetError returns the Error field if non-nil, zero value otherwise.

### GetErrorOk

`func (o *JobExecutionResponse) GetErrorOk() (*string, bool)`

GetErrorOk returns a tuple with the Error field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetError

`func (o *JobExecutionResponse) SetError(v string)`

SetError sets Error field to given value.

### HasError

`func (o *JobExecutionResponse) HasError() bool`

HasError returns a boolean if a field has been set.

### SetErrorNil

`func (o *JobExecutionResponse) SetErrorNil(b bool)`

 SetErrorNil sets the value for Error to be an explicit nil

### UnsetError
`func (o *JobExecutionResponse) UnsetError()`

UnsetError ensures that no value is present for Error, not even an explicit nil
### GetExitCode

`func (o *JobExecutionResponse) GetExitCode() int32`

GetExitCode returns the ExitCode field if non-nil, zero value otherwise.

### GetExitCodeOk

`func (o *JobExecutionResponse) GetExitCodeOk() (*int32, bool)`

GetExitCodeOk returns a tuple with the ExitCode field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetExitCode

`func (o *JobExecutionResponse) SetExitCode(v int32)`

SetExitCode sets ExitCode field to given value.

### HasExitCode

`func (o *JobExecutionResponse) HasExitCode() bool`

HasExitCode returns a boolean if a field has been set.

### SetExitCodeNil

`func (o *JobExecutionResponse) SetExitCodeNil(b bool)`

 SetExitCodeNil sets the value for ExitCode to be an explicit nil

### UnsetExitCode
`func (o *JobExecutionResponse) UnsetExitCode()`

UnsetExitCode ensures that no value is present for ExitCode, not even an explicit nil
### GetId

`func (o *JobExecutionResponse) GetId() string`

GetId returns the Id field if non-nil, zero value otherwise.

### GetIdOk

`func (o *JobExecutionResponse) GetIdOk() (*string, bool)`

GetIdOk returns a tuple with the Id field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetId

`func (o *JobExecutionResponse) SetId(v string)`

SetId sets Id field to given value.


### GetJobName

`func (o *JobExecutionResponse) GetJobName() string`

GetJobName returns the JobName field if non-nil, zero value otherwise.

### GetJobNameOk

`func (o *JobExecutionResponse) GetJobNameOk() (*string, bool)`

GetJobNameOk returns a tuple with the JobName field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetJobName

`func (o *JobExecutionResponse) SetJobName(v string)`

SetJobName sets JobName field to given value.


### GetLogs

`func (o *JobExecutionResponse) GetLogs() string`

GetLogs returns the Logs field if non-nil, zero value otherwise.

### GetLogsOk

`func (o *JobExecutionResponse) GetLogsOk() (*string, bool)`

GetLogsOk returns a tuple with the Logs field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetLogs

`func (o *JobExecutionResponse) SetLogs(v string)`

SetLogs sets Logs field to given value.

### HasLogs

`func (o *JobExecutionResponse) HasLogs() bool`

HasLogs returns a boolean if a field has been set.

### SetLogsNil

`func (o *JobExecutionResponse) SetLogsNil(b bool)`

 SetLogsNil sets the value for Logs to be an explicit nil

### UnsetLogs
`func (o *JobExecutionResponse) UnsetLogs()`

UnsetLogs ensures that no value is present for Logs, not even an explicit nil
### GetStartedAt

`func (o *JobExecutionResponse) GetStartedAt() string`

GetStartedAt returns the StartedAt field if non-nil, zero value otherwise.

### GetStartedAtOk

`func (o *JobExecutionResponse) GetStartedAtOk() (*string, bool)`

GetStartedAtOk returns a tuple with the StartedAt field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetStartedAt

`func (o *JobExecutionResponse) SetStartedAt(v string)`

SetStartedAt sets StartedAt field to given value.

### HasStartedAt

`func (o *JobExecutionResponse) HasStartedAt() bool`

HasStartedAt returns a boolean if a field has been set.

### SetStartedAtNil

`func (o *JobExecutionResponse) SetStartedAtNil(b bool)`

 SetStartedAtNil sets the value for StartedAt to be an explicit nil

### UnsetStartedAt
`func (o *JobExecutionResponse) UnsetStartedAt()`

UnsetStartedAt ensures that no value is present for StartedAt, not even an explicit nil
### GetStatus

`func (o *JobExecutionResponse) GetStatus() string`

GetStatus returns the Status field if non-nil, zero value otherwise.

### GetStatusOk

`func (o *JobExecutionResponse) GetStatusOk() (*string, bool)`

GetStatusOk returns a tuple with the Status field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetStatus

`func (o *JobExecutionResponse) SetStatus(v string)`

SetStatus sets Status field to given value.


### GetTrigger

`func (o *JobExecutionResponse) GetTrigger() string`

GetTrigger returns the Trigger field if non-nil, zero value otherwise.

### GetTriggerOk

`func (o *JobExecutionResponse) GetTriggerOk() (*string, bool)`

GetTriggerOk returns a tuple with the Trigger field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetTrigger

`func (o *JobExecutionResponse) SetTrigger(v string)`

SetTrigger sets Trigger field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


