# DaemonEventOneOf2

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**At** | **time.Time** | Wall-clock time. | 
**ContainerId** | Pointer to **string** | Container id, populated for &#x60;Connect&#x60;/&#x60;Disconnect&#x60;. | [optional] 
**Driver** | Pointer to **string** | Network driver (e.g. &#x60;bridge&#x60;, &#x60;overlay&#x60;). | [optional] 
**Id** | **string** | Network identifier (registry id). | 
**Kind** | [**NetworkEventKind**](NetworkEventKind.md) |  | 
**Name** | **string** | Human-readable network name. | 
**Resource** | **string** |  | 

## Methods

### NewDaemonEventOneOf2

`func NewDaemonEventOneOf2(at time.Time, id string, kind NetworkEventKind, name string, resource string, ) *DaemonEventOneOf2`

NewDaemonEventOneOf2 instantiates a new DaemonEventOneOf2 object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewDaemonEventOneOf2WithDefaults

`func NewDaemonEventOneOf2WithDefaults() *DaemonEventOneOf2`

NewDaemonEventOneOf2WithDefaults instantiates a new DaemonEventOneOf2 object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetAt

`func (o *DaemonEventOneOf2) GetAt() time.Time`

GetAt returns the At field if non-nil, zero value otherwise.

### GetAtOk

`func (o *DaemonEventOneOf2) GetAtOk() (*time.Time, bool)`

GetAtOk returns a tuple with the At field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetAt

`func (o *DaemonEventOneOf2) SetAt(v time.Time)`

SetAt sets At field to given value.


### GetContainerId

`func (o *DaemonEventOneOf2) GetContainerId() string`

GetContainerId returns the ContainerId field if non-nil, zero value otherwise.

### GetContainerIdOk

`func (o *DaemonEventOneOf2) GetContainerIdOk() (*string, bool)`

GetContainerIdOk returns a tuple with the ContainerId field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetContainerId

`func (o *DaemonEventOneOf2) SetContainerId(v string)`

SetContainerId sets ContainerId field to given value.

### HasContainerId

`func (o *DaemonEventOneOf2) HasContainerId() bool`

HasContainerId returns a boolean if a field has been set.

### GetDriver

`func (o *DaemonEventOneOf2) GetDriver() string`

GetDriver returns the Driver field if non-nil, zero value otherwise.

### GetDriverOk

`func (o *DaemonEventOneOf2) GetDriverOk() (*string, bool)`

GetDriverOk returns a tuple with the Driver field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetDriver

`func (o *DaemonEventOneOf2) SetDriver(v string)`

SetDriver sets Driver field to given value.

### HasDriver

`func (o *DaemonEventOneOf2) HasDriver() bool`

HasDriver returns a boolean if a field has been set.

### GetId

`func (o *DaemonEventOneOf2) GetId() string`

GetId returns the Id field if non-nil, zero value otherwise.

### GetIdOk

`func (o *DaemonEventOneOf2) GetIdOk() (*string, bool)`

GetIdOk returns a tuple with the Id field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetId

`func (o *DaemonEventOneOf2) SetId(v string)`

SetId sets Id field to given value.


### GetKind

`func (o *DaemonEventOneOf2) GetKind() NetworkEventKind`

GetKind returns the Kind field if non-nil, zero value otherwise.

### GetKindOk

`func (o *DaemonEventOneOf2) GetKindOk() (*NetworkEventKind, bool)`

GetKindOk returns a tuple with the Kind field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetKind

`func (o *DaemonEventOneOf2) SetKind(v NetworkEventKind)`

SetKind sets Kind field to given value.


### GetName

`func (o *DaemonEventOneOf2) GetName() string`

GetName returns the Name field if non-nil, zero value otherwise.

### GetNameOk

`func (o *DaemonEventOneOf2) GetNameOk() (*string, bool)`

GetNameOk returns a tuple with the Name field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetName

`func (o *DaemonEventOneOf2) SetName(v string)`

SetName sets Name field to given value.


### GetResource

`func (o *DaemonEventOneOf2) GetResource() string`

GetResource returns the Resource field if non-nil, zero value otherwise.

### GetResourceOk

`func (o *DaemonEventOneOf2) GetResourceOk() (*string, bool)`

GetResourceOk returns a tuple with the Resource field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetResource

`func (o *DaemonEventOneOf2) SetResource(v string)`

SetResource sets Resource field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


